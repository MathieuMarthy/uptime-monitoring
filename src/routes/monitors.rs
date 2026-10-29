use crate::error::AppError;
use crate::forms::monitor::{FieldErrors, MonitorForm};
use crate::models::{Channel, Monitor, Session, is_unique_violation};
use crate::routes::{PathId, form_or_invalid};
use crate::state::AppState;
use crate::views::monitors::{DetailTemplate, FormTemplate, IndexTemplate};
use axum::extract::State;
use axum::extract::rejection::FormRejection;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};

const DUPLICATE_NAME: &str = "This name is already used.";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/monitors/new", get(new_form).post(create))
        .route("/monitors/{id}", get(detail))
        .route("/monitors/{id}/edit", get(edit_form).post(update))
        .route("/monitors/{id}/delete", post(delete))
        .route("/monitors/{id}/toggle-pause", post(toggle_pause))
}

async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<IndexTemplate, AppError> {
    IndexTemplate::load(&state.db, session.csrf_token).await
}

async fn detail(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
) -> Result<DetailTemplate, AppError> {
    DetailTemplate::load(&state.db, id, session.csrf_token).await
}

async fn new_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<FormTemplate, AppError> {
    let channels = Channel::all(&state.db).await?;
    let mut form = MonitorForm::new_defaults();
    let enabled: Vec<i64> = channels
        .iter()
        .filter(|c| c.enabled)
        .map(|c| c.id)
        .collect();
    form.set_channels(&enabled);
    Ok(create_page(
        form,
        FieldErrors::new(),
        session.csrf_token,
        channels,
    ))
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
            let channels = Channel::all(&state.db).await?;
            return Ok(create_page(form, errors, session.csrf_token, channels).into_response());
        }
    };

    match Monitor::create(&state.db, &input).await {
        Ok(id) => {
            state.engine.reload(id);
            Ok(Redirect::to(&format!("/monitors/{id}")).into_response())
        }
        Err(e) if is_unique_violation(&e) => {
            let channels = Channel::all(&state.db).await?;
            Ok(create_page(form, name_taken(), session.csrf_token, channels).into_response())
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
    let channels = Channel::all(&state.db).await?;
    Ok(edit_page(
        id,
        form,
        FieldErrors::new(),
        session.csrf_token,
        channels,
    ))
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
            let channels = Channel::all(&state.db).await?;
            return Ok(edit_page(id, form, errors, session.csrf_token, channels).into_response());
        }
    };

    match Monitor::update(&state.db, id, &input).await {
        Ok(true) => {
            state.engine.reload(id);
            Ok(Redirect::to(&format!("/monitors/{id}")).into_response())
        }
        Ok(false) => Err(AppError::NotFound),
        Err(e) if is_unique_violation(&e) => {
            let channels = Channel::all(&state.db).await?;
            Ok(edit_page(id, form, name_taken(), session.csrf_token, channels).into_response())
        }
        Err(e) => Err(e.into()),
    }
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

fn create_page(
    form: MonitorForm,
    errors: FieldErrors,
    csrf_token: String,
    channels: Vec<Channel>,
) -> FormTemplate {
    FormTemplate {
        form,
        errors,
        csrf_token,
        action_url: "/monitors/new".to_string(),
        cancel_url: "/".to_string(),
        monitor_id: None,
        channels,
    }
}

fn edit_page(
    id: i64,
    form: MonitorForm,
    errors: FieldErrors,
    csrf_token: String,
    channels: Vec<Channel>,
) -> FormTemplate {
    FormTemplate {
        form,
        errors,
        csrf_token,
        action_url: format!("/monitors/{id}/edit"),
        cancel_url: format!("/monitors/{id}"),
        monitor_id: Some(id),
        channels,
    }
}

fn name_taken() -> FieldErrors {
    FieldErrors::from([("name", DUPLICATE_NAME.to_string())])
}
