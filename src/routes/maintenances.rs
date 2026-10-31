use crate::error::AppError;
use crate::forms::maintenance::MaintenanceForm;
use crate::forms::monitor::FieldErrors;
use crate::models::{Maintenance, MaintenanceInput, Monitor, Session, Settings};
use crate::routes::{PathId, form_or_invalid};
use crate::state::AppState;
use crate::time::now_ms;
use crate::views::maintenances::{FormTemplate, IndexTemplate};
use crate::windows::{MAX_WEEKLY_MINUTES, Schedule};
use axum::extract::State;
use axum::extract::rejection::FormRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/maintenances", get(index))
        .route("/maintenances/new", get(new_form).post(create))
        .route("/maintenances/{id}/edit", get(edit_form).post(update))
        .route("/maintenances/{id}/delete", post(delete))
        .route("/monitors/{id}/maintenance", post(start_now))
}

async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<IndexTemplate, AppError> {
    IndexTemplate::load(&state.db, session.csrf_token).await
}

async fn new_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<FormTemplate, AppError> {
    let form = MaintenanceForm::new_defaults();
    page(&state, None, form, FieldErrors::new(), session.csrf_token).await
}

async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<MaintenanceForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    match validate(&state, &form).await? {
        Ok(input) => {
            Maintenance::create(&state.db, &input).await?;
            changed(&state).await?;
            Ok(Redirect::to("/maintenances").into_response())
        }
        Err(errors) => Ok(page(&state, None, form, errors, session.csrf_token)
            .await?
            .into_response()),
    }
}

async fn edit_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
) -> Result<FormTemplate, AppError> {
    let (maintenance, ids) = Maintenance::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let tz = Settings::load(&state.db).await?.tz();
    let form = MaintenanceForm::from_maintenance(&maintenance, &ids, &tz);
    page(
        &state,
        Some(id),
        form,
        FieldErrors::new(),
        session.csrf_token,
    )
    .await
}

async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
    form: Result<Form<MaintenanceForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    match validate(&state, &form).await? {
        Ok(input) => {
            if !Maintenance::update(&state.db, id, &input).await? {
                return Err(AppError::NotFound);
            }
            changed(&state).await?;
            Ok(Redirect::to("/maintenances").into_response())
        }
        Err(errors) => Ok(page(&state, Some(id), form, errors, session.csrf_token)
            .await?
            .into_response()),
    }
}

async fn delete(State(state): State<AppState>, PathId(id): PathId) -> Result<Redirect, AppError> {
    if !Maintenance::delete(&state.db, id).await? {
        return Err(AppError::NotFound);
    }
    changed(&state).await?;
    Ok(Redirect::to("/maintenances"))
}

#[derive(Deserialize)]
struct NowForm {
    minutes: String,
}

/// The "Maintenance now" button of a monitor's page: a one-off window on that monitor, from now.
async fn start_now(
    State(state): State<AppState>,
    PathId(id): PathId,
    form: Result<Form<NowForm>, FormRejection>,
) -> Result<Redirect, AppError> {
    let form = form_or_invalid(form)?;
    let minutes = form
        .minutes
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|m| (1..=MAX_WEEKLY_MINUTES).contains(m))
        .ok_or(AppError::InvalidRequest(StatusCode::BAD_REQUEST))?;
    Monitor::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let now = now_ms();
    let input = MaintenanceInput {
        title: "Maintenance".to_string(),
        schedule: Schedule::Once {
            starts_at: now,
            ends_at: now + minutes * 60_000,
        },
        all_monitors: false,
        show_on_status_pages: false,
        monitor_ids: vec![id],
    };
    Maintenance::create(&state.db, &input).await?;
    changed(&state).await?;
    Ok(Redirect::to(&format!("/monitors/{id}")))
}

/// After any change to a window: the engine and the status pages see it at once.
async fn changed(state: &AppState) -> Result<(), AppError> {
    state.maintenances.reload(&state.db).await?;
    state.public_pages.clear();
    Ok(())
}

async fn validate(
    state: &AppState,
    form: &MaintenanceForm,
) -> Result<Result<MaintenanceInput, FieldErrors>, AppError> {
    let tz = Settings::load(&state.db).await?.tz();
    let monitors: Vec<(i64, Option<String>)> = Monitor::summaries(&state.db)
        .await?
        .into_iter()
        .map(|m| (m.id, m.group_name))
        .collect();
    Ok(form.validate(&tz, &monitors))
}

async fn page(
    state: &AppState,
    id: Option<i64>,
    form: MaintenanceForm,
    errors: FieldErrors,
    csrf_token: String,
) -> Result<FormTemplate, AppError> {
    Ok(FormTemplate {
        form,
        errors,
        csrf_token,
        action_url: match id {
            Some(id) => format!("/maintenances/{id}/edit"),
            None => "/maintenances/new".to_string(),
        },
        maintenance_id: id,
        monitors: Monitor::summaries(&state.db).await?,
        groups: Monitor::group_names(&state.db).await?,
        timezone: Settings::load(&state.db).await?.timezone,
    })
}
