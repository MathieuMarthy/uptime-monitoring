use crate::error::AppError;
use crate::forms::channel::ChannelForm;
use crate::forms::monitor::FieldErrors;
use crate::models::{Channel, Session, is_unique_violation};
use crate::notify::channel::ChannelConfig;
use crate::routes::{PathId, form_or_invalid};
use crate::state::AppState;
use crate::views::alerts::{FormTemplate, IndexTemplate};
use axum::extract::State;
use axum::extract::rejection::FormRejection;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};

const DUPLICATE_NAME: &str = "This name is already used.";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/alerts", get(index))
        .route("/alerts/new", get(new_form).post(create))
        .route("/alerts/{id}/edit", get(edit_form).post(update))
        .route("/alerts/{id}/delete", post(delete))
        .route("/alerts/{id}/test", post(test))
}

async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<IndexTemplate, AppError> {
    IndexTemplate::load(&state.db, session.csrf_token).await
}

async fn new_form(Extension(session): Extension<Session>) -> FormTemplate {
    page(
        None,
        ChannelForm::new_defaults(),
        FieldErrors::new(),
        session.csrf_token,
        None,
    )
}

async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<ChannelForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    let input = match form.validate(None) {
        Ok(input) => input,
        Err(errors) => {
            return Ok(page(None, form, errors, session.csrf_token, None).into_response());
        }
    };
    match Channel::create(&state.db, &input).await {
        Ok(_) => Ok(Redirect::to("/alerts").into_response()),
        Err(e) if is_unique_violation(&e) => {
            Ok(page(None, form, name_taken(), session.csrf_token, None).into_response())
        }
        Err(e) => Err(e.into()),
    }
}

async fn edit_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
) -> Result<FormTemplate, AppError> {
    let (channel, config) = load(&state, id).await?;
    let form = ChannelForm::from_config(&channel.name, &config, channel.enabled);
    Ok(page(
        Some(id),
        form,
        FieldErrors::new(),
        session.csrf_token,
        Some(&config),
    ))
}

async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
    form: Result<Form<ChannelForm>, FormRejection>,
) -> Result<Response, AppError> {
    let (_, existing) = load(&state, id).await?;
    let form = form_or_invalid(form)?;
    let input = match form.validate(Some(&existing)) {
        Ok(input) => input,
        Err(errors) => {
            return Ok(
                page(Some(id), form, errors, session.csrf_token, Some(&existing)).into_response(),
            );
        }
    };
    match Channel::update(&state.db, id, &input).await {
        Ok(true) => Ok(Redirect::to("/alerts").into_response()),
        Ok(false) => Err(AppError::NotFound),
        Err(e) if is_unique_violation(&e) => Ok(page(
            Some(id),
            form,
            name_taken(),
            session.csrf_token,
            Some(&existing),
        )
        .into_response()),
        Err(e) => Err(e.into()),
    }
}

async fn delete(State(state): State<AppState>, PathId(id): PathId) -> Result<Redirect, AppError> {
    if Channel::delete(&state.db, id).await? {
        Ok(Redirect::to("/alerts"))
    } else {
        Err(AppError::NotFound)
    }
}

/// Sends a test message and waits for the result, which the delivery log then shows.
async fn test(State(state): State<AppState>, PathId(id): PathId) -> Result<Redirect, AppError> {
    let channel = Channel::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;
    state.notifier.send_test(&state.db, &channel).await?;
    Ok(Redirect::to("/alerts"))
}

async fn load(state: &AppState, id: i64) -> Result<(Channel, ChannelConfig), AppError> {
    let channel = Channel::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let config = channel.parsed_config().map_err(|e| {
        AppError::Internal(format!("channel {id} has an invalid config: {e}").into())
    })?;
    Ok((channel, config))
}

fn page(
    id: Option<i64>,
    form: ChannelForm,
    errors: FieldErrors,
    csrf_token: String,
    existing: Option<&ChannelConfig>,
) -> FormTemplate {
    let has_token = matches!(existing, Some(ChannelConfig::Ntfy(c)) if c.token.is_some());
    FormTemplate {
        form,
        errors,
        csrf_token,
        action_url: match id {
            Some(id) => format!("/alerts/{id}/edit"),
            None => "/alerts/new".to_string(),
        },
        channel_id: id,
        has_token,
    }
}

fn name_taken() -> FieldErrors {
    FieldErrors::from([("name", DUPLICATE_NAME.to_string())])
}
