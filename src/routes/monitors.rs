use crate::error::AppError;
use crate::forms::monitor::{FieldErrors, MonitorForm};
use crate::models::{Monitor, Session, is_unique_violation};
use crate::routes::{MonitorId, form_or_invalid};
use crate::state::AppState;
use crate::views::monitors::{DetailTemplate, FormTemplate, IndexTemplate, MonitorView};
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
    Ok(IndexTemplate::new(
        MonitorView::load_all(&state.db).await?,
        session.csrf_token,
    ))
}

async fn detail(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    MonitorId(id): MonitorId,
) -> Result<DetailTemplate, AppError> {
    Ok(DetailTemplate {
        view: MonitorView::load(&state.db, id).await?,
        csrf_token: session.csrf_token,
    })
}

async fn new_form(Extension(session): Extension<Session>) -> FormTemplate {
    create_page(
        MonitorForm::new_defaults(),
        FieldErrors::new(),
        session.csrf_token,
    )
}

async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<MonitorForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    let input = match form.validate() {
        Ok(input) => input,
        Err(errors) => return Ok(create_page(form, errors, session.csrf_token).into_response()),
    };

    match Monitor::create(&state.db, &input).await {
        Ok(id) => Ok(Redirect::to(&format!("/monitors/{id}")).into_response()),
        Err(e) if is_unique_violation(&e) => {
            Ok(create_page(form, name_taken(), session.csrf_token).into_response())
        }
        Err(e) => Err(e.into()),
    }
}

async fn edit_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    MonitorId(id): MonitorId,
) -> Result<FormTemplate, AppError> {
    let monitor = Monitor::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(edit_page(
        id,
        MonitorForm::from_monitor(&monitor),
        FieldErrors::new(),
        session.csrf_token,
    ))
}

async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    MonitorId(id): MonitorId,
    form: Result<Form<MonitorForm>, FormRejection>,
) -> Result<Response, AppError> {
    if Monitor::find(&state.db, id).await?.is_none() {
        return Err(AppError::NotFound);
    }

    let form = form_or_invalid(form)?;
    let input = match form.validate() {
        Ok(input) => input,
        Err(errors) => return Ok(edit_page(id, form, errors, session.csrf_token).into_response()),
    };

    match Monitor::update(&state.db, id, &input).await {
        Ok(true) => Ok(Redirect::to(&format!("/monitors/{id}")).into_response()),
        Ok(false) => Err(AppError::NotFound),
        Err(e) if is_unique_violation(&e) => {
            Ok(edit_page(id, form, name_taken(), session.csrf_token).into_response())
        }
        Err(e) => Err(e.into()),
    }
}

async fn delete(
    State(state): State<AppState>,
    MonitorId(id): MonitorId,
) -> Result<Redirect, AppError> {
    if Monitor::delete(&state.db, id).await? {
        Ok(Redirect::to("/"))
    } else {
        Err(AppError::NotFound)
    }
}

async fn toggle_pause(
    State(state): State<AppState>,
    MonitorId(id): MonitorId,
) -> Result<Redirect, AppError> {
    if Monitor::toggle_pause(&state.db, id).await? {
        Ok(Redirect::to(&format!("/monitors/{id}")))
    } else {
        Err(AppError::NotFound)
    }
}

fn create_page(form: MonitorForm, errors: FieldErrors, csrf_token: String) -> FormTemplate {
    FormTemplate {
        form,
        errors,
        csrf_token,
        action_url: "/monitors/new".to_string(),
        cancel_url: "/".to_string(),
        monitor_id: None,
    }
}

fn edit_page(id: i64, form: MonitorForm, errors: FieldErrors, csrf_token: String) -> FormTemplate {
    FormTemplate {
        form,
        errors,
        csrf_token,
        action_url: format!("/monitors/{id}/edit"),
        cancel_url: format!("/monitors/{id}"),
        monitor_id: Some(id),
    }
}

fn name_taken() -> FieldErrors {
    FieldErrors::from([("name", DUPLICATE_NAME.to_string())])
}
