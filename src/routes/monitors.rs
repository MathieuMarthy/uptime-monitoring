use crate::error::AppError;
use crate::forms::monitor::{FieldErrors, MonitorForm, PUSH, clean_group};
use crate::models::{Channel, Monitor, Session, is_unique_violation};
use crate::routes::{PathId, form_or_invalid, parse_id};
use crate::state::AppState;
use crate::views::monitors::{
    BulkDone, DashboardFilter, DetailTemplate, FormTemplate, IndexTemplate, Range, test_summary,
};
use axum::extract::rejection::FormRejection;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use std::collections::HashMap;

/// The Test button's checks stop after this long, whatever the monitor's timeout.
const TEST_TIMEOUT_S: i64 = 30;
const DUPLICATE_NAME: &str = "This name is already used.";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/monitors/new", get(new_form).post(create))
        .route("/monitors/bulk", post(bulk))
        .route("/monitors/new/test", post(test_new))
        .route("/monitors/{id}", get(detail))
        .route("/monitors/{id}/edit", get(edit_form).post(update))
        .route("/monitors/{id}/edit/test", post(test_edit))
        .route("/monitors/{id}/duplicate", get(duplicate_form))
        .route("/monitors/{id}/delete", post(delete))
        .route("/monitors/{id}/toggle-pause", post(toggle_pause))
        .route("/monitors/{id}/push-token", post(renew_push_token))
}

/// Reads the parameters one by one, keeping the first of a repeated key: a hand-edited link
/// loses only what it got wrong.
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    RawQuery(query): RawQuery,
) -> Result<IndexTemplate, AppError> {
    let mut params: HashMap<String, String> = HashMap::new();
    for (key, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        params.entry(key.into_owned()).or_insert(value.into_owned());
    }
    let param = |key: &str| params.get(key).map(String::as_str);
    let filter = DashboardFilter::parse(
        Range::parse(param("range")),
        param("q"),
        param("status"),
        param("group"),
    );
    let done = BulkDone::parse(param("done"), param("count"));
    IndexTemplate::load(&state.db, filter, done, session.csrf_token).await
}

/// One action on the monitors ticked on the dashboard (`id`, repeated), then back to the dashboard
/// as it was filtered (`back`) with a notice of how many changed. Unknown ids are skipped.
async fn bulk(State(state): State<AppState>, body: String) -> Result<Redirect, AppError> {
    let invalid = || AppError::InvalidRequest(StatusCode::BAD_REQUEST);
    let (mut action, mut ids, mut group, mut channel, mut back) =
        (String::new(), Vec::new(), String::new(), None, None);
    for (key, value) in form_urlencoded::parse(body.as_bytes()) {
        match key.as_ref() {
            "action" => action = value.into_owned(),
            "id" => ids.push(value.parse::<i64>().map_err(|_| invalid())?),
            "group" => group = value.into_owned(),
            "channel" => channel = Some(value.into_owned()),
            "back" => back = Some(value.into_owned()),
            _ => {}
        }
    }
    let done = if ids.is_empty() {
        BulkDone::NoneSelected
    } else {
        match action.as_str() {
            "pause" | "resume" => {
                let changed = Monitor::set_paused(&state.db, &ids, action == "pause").await?;
                changed.iter().for_each(|&id| state.engine.reload(id));
                let count = changed.len() as u64;
                if action == "pause" {
                    BulkDone::Paused(count)
                } else {
                    BulkDone::Resumed(count)
                }
            }
            "delete" => {
                let deleted = Monitor::delete_many(&state.db, &ids).await?;
                deleted.iter().for_each(|&id| state.engine.reload(id));
                BulkDone::Deleted(deleted.len() as u64)
            }
            "group" => match clean_group(&group) {
                Ok(group) => {
                    BulkDone::Moved(Monitor::set_group(&state.db, &ids, group.as_deref()).await?)
                }
                Err(error) => BulkDone::InvalidGroup(error),
            },
            "link" => {
                let channel = parse_channel(channel)?;
                BulkDone::Linked(Channel::link_monitors(&state.db, channel, &ids).await?)
            }
            "unlink" => {
                let channel = parse_channel(channel)?;
                BulkDone::Unlinked(Channel::unlink_monitors(&state.db, channel, &ids).await?)
            }
            _ => return Err(invalid()),
        }
    };
    // Only a dashboard address: anything else could send the browser off the site. Visible ASCII
    // only, as the browser encodes it: a header value cannot hold the rest.
    let back = back
        .filter(|b| b == "/" || b.starts_with("/?"))
        .filter(|b| b.bytes().all(|c| c.is_ascii_graphic()))
        .unwrap_or_else(|| "/".to_string());
    let separator = if back.contains('?') { '&' } else { '?' };
    Ok(Redirect::to(&format!("{back}{separator}{}", done.query())))
}

async fn detail(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
) -> Result<DetailTemplate, AppError> {
    DetailTemplate::load(
        &state.db,
        id,
        session.csrf_token,
        state.public_url.as_deref(),
    )
    .await
}

async fn new_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<FormTemplate, AppError> {
    let mut page = page(
        &state,
        None,
        MonitorForm::new_defaults(),
        FieldErrors::new(),
        session.csrf_token,
    )
    .await?;
    let enabled: Vec<i64> = page
        .channels
        .iter()
        .filter(|c| c.enabled)
        .map(|c| c.id)
        .collect();
    page.form.set_channels(&enabled);
    Ok(page)
}

async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<MonitorForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    let input = match form.validate() {
        Ok(input) => input,
        Err(errors) => {
            return Ok(page(&state, None, form, errors, session.csrf_token)
                .await?
                .into_response());
        }
    };

    match Monitor::create(&state.db, &input).await {
        Ok(id) => {
            state.engine.reload(id);
            Ok(Redirect::to(&format!("/monitors/{id}")).into_response())
        }
        Err(e) if is_unique_violation(&e) => {
            Ok(page(&state, None, form, name_taken(), session.csrf_token)
                .await?
                .into_response())
        }
        Err(e) => Err(e.into()),
    }
}

async fn edit_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
) -> Result<FormTemplate, AppError> {
    let monitor = Monitor::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut form = MonitorForm::from_monitor(&monitor);
    form.set_channels(&Channel::ids_for_monitor(&state.db, id).await?);
    page(
        &state,
        Some(id),
        form,
        FieldErrors::new(),
        session.csrf_token,
    )
    .await
}

/// The create form, filled from another monitor: configuration, group and channels, active.
/// Nothing is saved until it is submitted; a push copy gets its own token then.
async fn duplicate_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
) -> Result<FormTemplate, AppError> {
    let monitor = Monitor::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut form = MonitorForm::from_monitor(&monitor);
    form.name = format!("{} (copy)", monitor.name);
    form.active = Some("on".to_string());
    form.copy_of = id.to_string();
    form.set_channels(&Channel::ids_for_monitor(&state.db, id).await?);
    page(&state, None, form, FieldErrors::new(), session.csrf_token).await
}

async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
    form: Result<Form<MonitorForm>, FormRejection>,
) -> Result<Response, AppError> {
    if Monitor::find(&state.db, id).await?.is_none() {
        return Err(AppError::NotFound);
    }

    let form = form_or_invalid(form)?;
    let input = match form.validate() {
        Ok(input) => input,
        Err(errors) => {
            return Ok(page(&state, Some(id), form, errors, session.csrf_token)
                .await?
                .into_response());
        }
    };

    match Monitor::update(&state.db, id, &input).await {
        Ok(true) => {
            state.engine.reload(id);
            Ok(Redirect::to(&format!("/monitors/{id}")).into_response())
        }
        Ok(false) => Err(AppError::NotFound),
        Err(e) if is_unique_violation(&e) => {
            Ok(
                page(&state, Some(id), form, name_taken(), session.csrf_token)
                    .await?
                    .into_response(),
            )
        }
        Err(e) => Err(e.into()),
    }
}

/// The Test button of the add form: one check with the values typed, nothing saved.
async fn test_new(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<MonitorForm>, FormRejection>,
) -> Result<FormTemplate, AppError> {
    let form = form_or_invalid(form)?;
    test_page(&state, None, form, session.csrf_token).await
}

/// The Test button of the edit form; the monitor keeps its stored configuration.
async fn test_edit(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
    form: Result<Form<MonitorForm>, FormRejection>,
) -> Result<FormTemplate, AppError> {
    if Monitor::find(&state.db, id).await?.is_none() {
        return Err(AppError::NotFound);
    }
    let form = form_or_invalid(form)?;
    test_page(&state, Some(id), form, session.csrf_token).await
}

/// Checks the form's values once when they are valid, and shows the form again with the result.
async fn test_page(
    state: &AppState,
    id: Option<i64>,
    form: MonitorForm,
    csrf_token: String,
) -> Result<FormTemplate, AppError> {
    let input = match form.validate() {
        Ok(input) => input,
        Err(errors) => return page(state, id, form, errors, csrf_token).await,
    };
    let result = if input.kind == PUSH {
        Err("A push monitor waits for its job: save it, then call its URL.".to_string())
    } else {
        match state.monitor_tests.clone().try_acquire_owned() {
            Ok(_permit) => {
                let mut monitor = Monitor::unsaved(&input);
                // The page waits for the answer: a reverse proxy may give up sooner.
                monitor.timeout_s = monitor.timeout_s.min(TEST_TIMEOUT_S);
                let outcome = state.form_checker.check(&monitor).await;
                test_summary(&outcome, input.tls_expiry_warn_days)
            }
            Err(_) => Err("Other tests are running: try again in a moment.".to_string()),
        }
    };
    let mut page = page(state, id, form, FieldErrors::new(), csrf_token).await?;
    page.test_result = Some(result);
    Ok(page)
}

async fn delete(State(state): State<AppState>, PathId(id): PathId) -> Result<Redirect, AppError> {
    if Monitor::delete(&state.db, id).await? {
        state.engine.reload(id);
        Ok(Redirect::to("/"))
    } else {
        Err(AppError::NotFound)
    }
}

async fn toggle_pause(
    State(state): State<AppState>,
    PathId(id): PathId,
) -> Result<Redirect, AppError> {
    if Monitor::toggle_pause(&state.db, id).await? {
        state.engine.reload(id);
        Ok(Redirect::to(&format!("/monitors/{id}")))
    } else {
        Err(AppError::NotFound)
    }
}

/// A new push URL; the old one stops working at once.
async fn renew_push_token(
    State(state): State<AppState>,
    PathId(id): PathId,
) -> Result<Redirect, AppError> {
    if Monitor::renew_push_token(&state.db, id).await? {
        Ok(Redirect::to(&format!("/monitors/{id}")))
    } else {
        Err(AppError::NotFound)
    }
}

/// The channel of a bulk link or unlink: only those actions read it.
fn parse_channel(channel: Option<String>) -> Result<i64, AppError> {
    channel
        .and_then(|c| c.parse().ok())
        .ok_or(AppError::InvalidRequest(StatusCode::BAD_REQUEST))
}

/// The create (`id` = `None`) or edit form, with every channel for the alert checkboxes: a
/// form saved without them would unlink all channels.
async fn page(
    state: &AppState,
    id: Option<i64>,
    form: MonitorForm,
    errors: FieldErrors,
    csrf_token: String,
) -> Result<FormTemplate, AppError> {
    // A copy's form goes back to its source, as long as that still exists.
    let copy_of = match parse_id(&form.copy_of) {
        Ok(source) if id.is_none() && Monitor::find(&state.db, source).await?.is_some() => {
            Some(source)
        }
        _ => None,
    };
    let (action_url, cancel_url) = match (id, copy_of) {
        (Some(id), _) => (format!("/monitors/{id}/edit"), format!("/monitors/{id}")),
        (None, Some(source)) => ("/monitors/new".to_string(), format!("/monitors/{source}")),
        (None, None) => ("/monitors/new".to_string(), "/".to_string()),
    };
    Ok(FormTemplate {
        form,
        errors,
        csrf_token,
        action_url,
        cancel_url,
        monitor_id: id,
        copy_of,
        channels: Channel::all(&state.db).await?,
        groups: Monitor::group_names(&state.db).await?,
        test_result: None,
    })
}

fn name_taken() -> FieldErrors {
    FieldErrors::from([("name", DUPLICATE_NAME.to_string())])
}
