use crate::error::AppError;
use crate::forms::monitor::{FieldErrors, MonitorForm};
use crate::models::{Monitor, is_unique_violation};
use crate::routes::parse_id;
use crate::state::AppState;
use crate::views::monitors::{DetailTemplate, FormTemplate, IndexTemplate, MonitorView};
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};

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

async fn index(State(state): State<AppState>) -> Result<IndexTemplate, AppError> {
    Ok(IndexTemplate::new(MonitorView::load_all(&state.db).await?))
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<DetailTemplate, AppError> {
    let id = parse_id(&id)?;
    Ok(DetailTemplate {
        view: MonitorView::load(&state.db, id).await?,
    })
}

async fn new_form() -> FormTemplate {
    create_page(MonitorForm::new_defaults(), FieldErrors::new())
}

async fn create(
    State(state): State<AppState>,
    Form(form): Form<MonitorForm>,
) -> Result<Response, AppError> {
    let input = match form.validate() {
        Ok(input) => input,
        Err(errors) => return Ok(create_page(form, errors).into_response()),
    };

    match Monitor::create(&state.db, &input).await {
        Ok(id) => Ok(Redirect::to(&format!("/monitors/{id}")).into_response()),
        Err(e) if is_unique_violation(&e) => Ok(create_page(form, name_taken()).into_response()),
        Err(e) => Err(e.into()),
    }
}

async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<FormTemplate, AppError> {
    let id = parse_id(&id)?;
    let monitor = Monitor::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(edit_page(
        id,
        MonitorForm::from_monitor(&monitor),
        FieldErrors::new(),
    ))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<MonitorForm>,
) -> Result<Response, AppError> {
    let id = parse_id(&id)?;
    if Monitor::find(&state.db, id).await?.is_none() {
        return Err(AppError::NotFound);
    }

    let input = match form.validate() {
        Ok(input) => input,
        Err(errors) => return Ok(edit_page(id, form, errors).into_response()),
    };

    match Monitor::update(&state.db, id, &input).await {
        Ok(true) => Ok(Redirect::to(&format!("/monitors/{id}")).into_response()),
        Ok(false) => Err(AppError::NotFound),
        Err(e) if is_unique_violation(&e) => Ok(edit_page(id, form, name_taken()).into_response()),
        Err(e) => Err(e.into()),
    }
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Redirect, AppError> {
    let id = parse_id(&id)?;
    if Monitor::delete(&state.db, id).await? {
        Ok(Redirect::to("/"))
    } else {
        Err(AppError::NotFound)
    }
}

async fn toggle_pause(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Redirect, AppError> {
    let id = parse_id(&id)?;
    if Monitor::toggle_pause(&state.db, id).await? {
        Ok(Redirect::to(&format!("/monitors/{id}")))
    } else {
        Err(AppError::NotFound)
    }
}

fn create_page(form: MonitorForm, errors: FieldErrors) -> FormTemplate {
    FormTemplate {
        form,
        errors,
        action_url: "/monitors/new".to_string(),
        cancel_url: "/".to_string(),
        monitor_id: None,
    }
}

fn edit_page(id: i64, form: MonitorForm, errors: FieldErrors) -> FormTemplate {
    FormTemplate {
        form,
        errors,
        action_url: format!("/monitors/{id}/edit"),
        cancel_url: format!("/monitors/{id}"),
        monitor_id: Some(id),
    }
}

fn name_taken() -> FieldErrors {
    FieldErrors::from([("name", DUPLICATE_NAME.to_string())])
}
