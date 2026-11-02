use crate::auth::totp::{self, decode_secret, otpauth_uri, qr_svg};
use crate::auth::{hash_password, verify_password};
use crate::error::AppError;
use crate::forms::monitor::FieldErrors;
use crate::forms::settings::{
    BlockingForm, DataForm, GeneralForm, PasswordForm, TotpDisableForm, TotpEnableForm,
};
use crate::i18n::t;
use crate::models::{Admin, Session, Settings, StorageUse};
use crate::routes::auth::{ClientIp, invalid_code, totp_code_ok, with_password_hashing};
use crate::routes::form_or_invalid;
use crate::state::AppState;
use crate::time::now_ms;
use crate::transfer::{Export, import};
use crate::views::settings::{ImportTemplate, Section, SettingsTemplate, TotpSetupTemplate};
use crate::{IMPORT_MAX_BODY_BYTES, IMPORT_PATH};
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::extract::rejection::{FormRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use serde::Deserialize;
use std::time::Instant;
use tokio::io::AsyncReadExt;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(index))
        .route("/settings/general", post(update_general))
        .route("/settings/data", post(update_data))
        .route("/settings/blocking", post(update_blocking))
        .route("/settings/password", post(change_password))
        .route("/settings/totp", get(totp_setup).post(enable_totp))
        .route("/settings/totp/disable", post(disable_totp))
        .route("/settings/sessions", post(end_other_sessions))
        .route("/settings/export", get(export))
        .route("/settings/backup", get(backup))
        .route(
            IMPORT_PATH,
            post(import).layer(DefaultBodyLimit::max(IMPORT_MAX_BODY_BYTES)),
        )
}

#[derive(Deserialize)]
struct ImportForm {
    #[serde(default)]
    document: String,
}

/// Adds the monitors, channels, pages and windows of an export or an Uptime Kuma backup.
async fn import(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<ImportForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    let (export, mut report) = match import::parse(&form.document) {
        Ok(parsed) => parsed,
        Err(message) => {
            let mut page = page(&state, session, Section::Backup).await?;
            page.errors.insert("document", message);
            page.document = form.document;
            return Ok(page.into_response());
        }
    };
    import::apply(&state.db, &export, &mut report).await?;
    state.engine.reload_staggered(&report.monitor_ids);
    state.maintenances.reload(&state.db).await?;
    state.public_pages.clear();
    Ok(ImportTemplate {
        csrf_token: session.csrf_token,
        report,
    }
    .into_response())
}

/// A copy of the whole database, streamed from disk.
async fn backup(State(state): State<AppState>) -> Result<Response, AppError> {
    let file = crate::db::backup(&state.db)
        .await
        .map_err(|e| AppError::Internal(Box::new(e)))?;
    // The copy (and its permit) lives as long as the download.
    let chunks = futures_util::stream::try_unfold(file, |mut backup| async move {
        let mut chunk = vec![0; 64 * 1024];
        let read = backup.file.read(&mut chunk).await?;
        chunk.truncate(read);
        Ok::<_, std::io::Error>((read > 0).then_some((chunk, backup)))
    });
    let name = format!("uptime-monitoring-{}.db", today(&state).await?);
    Ok(attachment(
        "application/vnd.sqlite3",
        &name,
        Body::from_stream(chunks),
    ))
}

/// The configuration as a JSON file, secrets included.
async fn export(State(state): State<AppState>) -> Result<Response, AppError> {
    let export = Export::load(&state.db).await?;
    let json =
        serde_json::to_string_pretty(&export).map_err(|e| AppError::Internal(Box::new(e)))?;
    Ok(attachment(
        "application/json",
        &format!("uptime-monitoring-{}.json", today(&state).await?),
        json,
    ))
}

/// The local date of the instance, for file names.
async fn today(state: &AppState) -> Result<String, AppError> {
    let tz = Settings::load(&state.db).await?.tz();
    Ok(jiff::Timestamp::now().to_zoned(tz).date().to_string())
}

fn attachment(content_type: &'static str, file_name: &str, body: impl Into<Body>) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file_name}\""),
            ),
        ],
        body.into(),
    )
        .into_response()
}

#[derive(Deserialize)]
struct SavedQuery {
    section: Option<String>,
    saved: Option<String>,
}

async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    query: Result<Query<SavedQuery>, QueryRejection>,
) -> Result<SettingsTemplate, AppError> {
    let query = query.ok().map(|Query(q)| q);
    let section = query
        .as_ref()
        .and_then(|q| q.section.as_deref())
        .and_then(Section::from_param)
        .unwrap_or_default();
    let mut page = page(&state, session, section).await?;
    page.saved = match query.and_then(|q| q.saved).as_deref() {
        Some("password") => Some(t("Password changed. Every other session is logged out.")),
        Some("totp") => Some(t(
            "Two-factor authentication is on. Every other session is logged out.",
        )),
        Some("totp-off") => Some(t("Two-factor authentication is off.")),
        Some("sessions") => Some(t("Every other session is logged out.")),
        Some(_) => Some(t("Settings saved.")),
        None => None,
    };
    Ok(page)
}

async fn page(
    state: &AppState,
    session: Session,
    section: Section,
) -> Result<SettingsTemplate, AppError> {
    let settings = Settings::load(&state.db).await?;
    let storage = StorageUse::load(&state.db).await?;
    // Shown in the Security section only.
    let other_sessions = match section {
        Section::Security => session.count_others(&state.db).await?,
        _ => 0,
    };
    let mut page = SettingsTemplate::new(&settings, storage, session.csrf_token, section);
    page.totp_enabled = Admin::find(&state.db)
        .await?
        .is_some_and(|admin| admin.totp_secret.is_some());
    page.other_sessions = other_sessions;
    Ok(page)
}

/// Logs out every session but this one, as after a password change.
async fn end_other_sessions(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, AppError> {
    if !session.end_others(&state.db).await? {
        // Another session logged this one out first.
        return Ok(Redirect::to("/login").into_response());
    }
    tracing::info!("Other admin sessions logged out.");
    Ok(Redirect::to("/settings?section=security&saved=sessions").into_response())
}

async fn update_general(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<GeneralForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    match form.validate() {
        Ok((timezone, language)) => {
            Settings::update_general(&state.db, &timezone, language).await?;
            state.language.set(language);
            // Weekly maintenance windows follow the instance timezone.
            state.maintenances.reload(&state.db).await?;
            state.public_pages.clear();
            Ok(Redirect::to("/settings?section=general&saved=general").into_response())
        }
        Err(errors) => {
            let mut page = page(&state, session, Section::General).await?;
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
            Settings::update_retention(
                &state.db,
                input.checks_retention_days,
                input.history_retention_days,
            )
            .await?;
            Ok(Redirect::to("/settings?section=data&saved=data").into_response())
        }
        Err(errors) => {
            let mut page = page(&state, session, Section::Data).await?;
            page.data = form;
            page.errors = errors;
            Ok(page.into_response())
        }
    }
}

async fn update_blocking(
    State(state): State<AppState>,
    form: Result<Form<BlockingForm>, FormRejection>,
) -> Result<Response, AppError> {
    let block = form_or_invalid(form)?.block_private_targets.is_some();
    Settings::update_blocking(&state.db, block).await?;
    state.guard.set_block_private(block);
    Ok(Redirect::to("/settings?section=security&saved=blocking").into_response())
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
            t("Current password is incorrect.").to_string(),
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
    Ok(Redirect::to("/settings?section=security&saved=password").into_response())
}

async fn password_errors(
    state: &AppState,
    session: Session,
    errors: FieldErrors,
) -> Result<Response, AppError> {
    let mut page = page(state, session, Section::Security).await?;
    page.errors = errors;
    Ok(page.into_response())
}

async fn totp_setup(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, AppError> {
    let admin = Admin::find(&state.db).await?.ok_or(AppError::NotFound)?;
    if admin.totp_secret.is_some() {
        return Ok(Redirect::to("/settings?section=security").into_response());
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
/// authenticator app has it. The password is asked too, as a session alone could otherwise lock
/// the admin out; a wrong one counts toward the client's login limit.
async fn enable_totp(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    ClientIp(ip): ClientIp,
    form: Result<Form<TotpEnableForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    let secret = form.secret.trim().to_ascii_uppercase();
    let bytes = decode_secret(&secret).ok_or(AppError::InvalidRequest(StatusCode::BAD_REQUEST))?;
    let admin = Admin::find(&state.db).await?.ok_or(AppError::NotFound)?;
    if admin.totp_secret.is_some() {
        return Ok(Redirect::to("/settings?section=security").into_response());
    }
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
    if !with_password_hashing(&state, move || verify_password(&current, &hash)).await? {
        tracing::warn!(%ip, "wrong current password on turning two-factor authentication on");
        return Ok(totp_setup_page(
            &admin,
            secret,
            session.csrf_token,
            Some(t("Current password is incorrect.")),
        )
        .into_response());
    }
    state.login_limiter.lock().unwrap().record_success(ip);
    let Some(step) = totp::verify(&bytes, &form.code, now_ms(), None) else {
        return Ok(
            totp_setup_page(&admin, secret, session.csrf_token, Some(invalid_code()))
                .into_response(),
        );
    };
    if !Admin::enable_totp(&state.db, &secret, step, &session.token_hash).await? {
        // Another change ended this session first.
        return Ok(Redirect::to("/login").into_response());
    }
    tracing::info!("Two-factor authentication turned on; other sessions ended.");
    Ok(Redirect::to("/settings?section=security&saved=totp").into_response())
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
        return Ok(Redirect::to("/settings?section=security").into_response());
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
            t("Current password is incorrect.").to_string(),
        );
    } else if !totp_code_ok(&state, &secret, admin.totp_last_step, &form.code).await? {
        errors.insert("totp_code", invalid_code().to_string());
    }
    if !errors.is_empty() {
        tracing::warn!(%ip, "failed attempt to turn two-factor authentication off");
        return password_errors(&state, session, errors).await;
    }
    state.login_limiter.lock().unwrap().record_success(ip);
    Admin::disable_totp(&state.db).await?;
    tracing::info!("Two-factor authentication turned off.");
    Ok(Redirect::to("/settings?section=security&saved=totp-off").into_response())
}
