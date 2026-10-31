use crate::auth::totp::{self, decode_secret, otpauth_uri, qr_svg};
use crate::auth::{hash_password, verify_password};
use crate::error::AppError;
use crate::forms::monitor::FieldErrors;
use crate::forms::settings::{
    DataForm, GeneralForm, PasswordForm, TotpDisableForm, TotpEnableForm,
};
use crate::models::{Admin, Session, Settings, StorageUse};
use crate::routes::auth::{ClientIp, INVALID_CODE, totp_code_ok, with_password_hashing};
use crate::routes::form_or_invalid;
use crate::state::AppState;
use crate::time::now_ms;
use crate::views::settings::{SettingsTemplate, TotpSetupTemplate};
use axum::extract::rejection::{FormRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::StatusCode;
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
        .route("/settings/totp", get(totp_setup).post(enable_totp))
        .route("/settings/totp/disable", post(disable_totp))
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
        Some("totp") => Some("Two-factor authentication is on. Every other session is logged out."),
        Some("totp-off") => Some("Two-factor authentication is off."),
        Some(_) => Some("Settings saved."),
        None => None,
    };
    Ok(page)
}

async fn page(state: &AppState, csrf_token: String) -> Result<SettingsTemplate, AppError> {
    let settings = Settings::load(&state.db).await?;
    let storage = StorageUse::load(&state.db).await?;
    let mut page = SettingsTemplate::new(&settings, storage, csrf_token);
    page.totp_enabled = Admin::find(&state.db)
        .await?
        .is_some_and(|admin| admin.totp_secret.is_some());
    Ok(page)
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
            // Weekly maintenance windows follow the instance timezone.
            state.maintenances.reload(&state.db).await?;
            state.public_pages.clear();
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

async fn totp_setup(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, AppError> {
    let admin = Admin::find(&state.db).await?.ok_or(AppError::NotFound)?;
    if admin.totp_secret.is_some() {
        return Ok(Redirect::to("/settings").into_response());
    }
    Ok(totp_setup_page(&admin, totp::generate_secret(), session.csrf_token, None).into_response())
}

fn totp_setup_page(
    admin: &Admin,
    secret: String,
    csrf_token: String,
    error: Option<&'static str>,
) -> TotpSetupTemplate {
    TotpSetupTemplate {
        csrf_token,
        qr_svg: qr_svg(&otpauth_uri(&secret, &admin.username)),
        secret,
        error,
    }
}

/// The secret comes back from the setup page: it is only stored once a code proves the
/// authenticator app has it.
async fn enable_totp(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<TotpEnableForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    let secret = form.secret.trim().to_ascii_uppercase();
    let bytes = decode_secret(&secret).ok_or(AppError::InvalidRequest(StatusCode::BAD_REQUEST))?;
    let admin = Admin::find(&state.db).await?.ok_or(AppError::NotFound)?;
    if admin.totp_secret.is_some() {
        return Ok(Redirect::to("/settings").into_response());
    }
    let Some(step) = totp::verify(&bytes, &form.code, now_ms(), None) else {
        return Ok(
            totp_setup_page(&admin, secret, session.csrf_token, Some(INVALID_CODE)).into_response(),
        );
    };
    if !Admin::enable_totp(&state.db, &secret, step, &session.token_hash).await? {
        // Another change ended this session first.
        return Ok(Redirect::to("/login").into_response());
    }
    tracing::info!("Two-factor authentication turned on; other sessions ended.");
    Ok(Redirect::to("/settings?saved=totp").into_response())
}

/// Like a password change: the attempt counts toward the client's login limit.
async fn disable_totp(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    ClientIp(ip): ClientIp,
    form: Result<Form<TotpDisableForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    let admin = Admin::find(&state.db).await?.ok_or(AppError::NotFound)?;
    let Some(secret) = admin.totp_secret.clone() else {
        return Ok(Redirect::to("/settings").into_response());
    };
    if !state
        .login_limiter
        .lock()
        .unwrap()
        .try_attempt(ip, Instant::now())
    {
        return Err(AppError::TooManyRequests);
    }
    let current = form.current_password.clone();
    let hash = admin.password_hash.clone();
    let password_ok =
        with_password_hashing(&state, move || verify_password(&current, &hash)).await?;
    let mut errors = FieldErrors::new();
    if !password_ok {
        errors.insert(
            "totp_password",
            "Current password is incorrect.".to_string(),
        );
    } else if !totp_code_ok(&state, &secret, admin.totp_last_step, &form.code).await? {
        errors.insert("totp_code", INVALID_CODE.to_string());
    }
    if !errors.is_empty() {
        tracing::warn!(%ip, "failed attempt to turn two-factor authentication off");
        return password_errors(&state, session, errors).await;
    }
    state.login_limiter.lock().unwrap().record_success(ip);
    Admin::disable_totp(&state.db).await?;
    tracing::info!("Two-factor authentication turned off.");
    Ok(Redirect::to("/settings?saved=totp-off").into_response())
}
