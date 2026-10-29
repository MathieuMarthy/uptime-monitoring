use crate::auth::{hash_password, verify_password};
use crate::error::AppError;
use crate::forms::monitor::FieldErrors;
use crate::forms::settings::{DataForm, GeneralForm, PasswordForm};
use crate::models::{Admin, Session, Settings, StorageUse};
use crate::routes::auth::{ClientIp, with_password_hashing};
use crate::routes::form_or_invalid;
use crate::state::AppState;
use crate::views::settings::SettingsTemplate;
use axum::extract::rejection::{FormRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use serde::Deserialize;
use std::time::Instant;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(index))
        .route("/settings/general", post(update_general))
        .route("/settings/data", post(update_data))
        .route("/settings/password", post(change_password))
}

#[derive(Deserialize)]
struct SavedQuery {
    saved: Option<String>,
}

async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    query: Result<Query<SavedQuery>, QueryRejection>,
) -> Result<SettingsTemplate, AppError> {
    let mut page = page(&state, session.csrf_token).await?;
    page.saved = match query.ok().and_then(|Query(q)| q.saved).as_deref() {
        Some("password") => Some("Password changed. Every other session is logged out."),
        Some(_) => Some("Settings saved."),
        None => None,
    };
    Ok(page)
}

async fn page(state: &AppState, csrf_token: String) -> Result<SettingsTemplate, AppError> {
    let settings = Settings::load(&state.db).await?;
    let storage = StorageUse::load(&state.db).await?;
    Ok(SettingsTemplate::new(&settings, storage, csrf_token))
}

async fn update_general(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<GeneralForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    match form.validate() {
        Ok(timezone) => {
            Settings::update_timezone(&state.db, &timezone).await?;
            Ok(Redirect::to("/settings?saved=general").into_response())
        }
        Err(errors) => {
            let mut page = page(&state, session.csrf_token).await?;
            page.errors = errors;
            Ok(page.into_response())
        }
    }
}

async fn update_data(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<DataForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    match form.validate() {
        Ok(input) => {
            Settings::update_data(
                &state.db,
                input.checks_retention_days,
                input.history_retention_days,
                input.block_private_targets,
            )
            .await?;
            state.guard.set_block_private(input.block_private_targets);
            Ok(Redirect::to("/settings?saved=data").into_response())
        }
        Err(errors) => {
            let mut page = page(&state, session.csrf_token).await?;
            page.data = form;
            page.errors = errors;
            Ok(page.into_response())
        }
    }
}

/// Like a login: a wrong current password counts toward the client's login attempts.
async fn change_password(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    ClientIp(ip): ClientIp,
    form: Result<Form<PasswordForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    if let Err(errors) = form.validate() {
        return password_errors(&state, session, errors).await;
    }
    if !state
        .login_limiter
        .lock()
        .unwrap()
        .try_attempt(ip, Instant::now())
    {
        return Err(AppError::TooManyRequests);
    }
    let admin = Admin::find(&state.db).await?.ok_or(AppError::NotFound)?;
    let current = form.current_password.clone();
    let verified_hash = admin.password_hash.clone();
    let current_ok = with_password_hashing(&state, move || {
        verify_password(&current, &admin.password_hash)
    })
    .await?;
    if !current_ok {
        tracing::warn!(%ip, "wrong current password on a password change");
        let mut errors = FieldErrors::new();
        errors.insert(
            "current_password",
            "Current password is incorrect.".to_string(),
        );
        return password_errors(&state, session, errors).await;
    }
    state.login_limiter.lock().unwrap().record_success(ip);

    let password = form.password.clone();
    let hash = with_password_hashing(&state, move || hash_password(&password))
        .await?
        .map_err(|e| AppError::Internal(e.to_string().into()))?;
    if !Admin::change_password(&state.db, &verified_hash, &hash, &session.token_hash).await? {
        // Another change ended this session or replaced the password first.
        return Ok(Redirect::to("/login").into_response());
    }
    tracing::info!("Admin password changed; other sessions ended.");
    Ok(Redirect::to("/settings?saved=password").into_response())
}

async fn password_errors(
    state: &AppState,
    session: Session,
    errors: FieldErrors,
) -> Result<Response, AppError> {
    let mut page = page(state, session.csrf_token).await?;
    page.errors = errors;
    Ok(page.into_response())
}
