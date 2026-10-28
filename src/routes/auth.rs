use crate::auth::{
    SESSION_COOKIE, cookie, expired_session_cookie, hash_password, print_setup_url, secrets_match,
    session_cookie, sha256_hex, verify_password,
};
use crate::error::AppError;
use crate::forms::auth::{LoginForm, SetupForm};
use crate::forms::monitor::FieldErrors;
use crate::models::{Admin, Session, is_unique_violation};
use crate::routes::form_or_invalid;
use crate::state::AppState;
use crate::views::auth::{LoginTemplate, SetupTemplate};
use axum::extract::rejection::{FormRejection, QueryRejection};
use axum::extract::{ConnectInfo, FromRequestParts, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Extension, Form, Router};
use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

const LOGIN_FAILED: &str = "Invalid username or password.";
const HASHING_WAIT: Duration = Duration::from_secs(5);

/// Routes reachable without a session.
pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/setup", get(setup_page).post(setup))
        .route("/login", get(login_page).post(login))
}

/// Routes behind the session guard.
pub fn protected_router() -> Router<AppState> {
    Router::new().route("/logout", axum::routing::post(logout))
}

#[derive(Deserialize)]
struct SetupQuery {
    token: Option<String>,
}

async fn setup_page(
    State(state): State<AppState>,
    query: Result<Query<SetupQuery>, QueryRejection>,
) -> Response {
    if !state.setup_pending() {
        return Redirect::to("/login").into_response();
    }
    let token = query
        .ok()
        .and_then(|Query(q)| q.token)
        .filter(|token| setup_token_matches(&state, token));
    SetupTemplate {
        token,
        username: String::new(),
        errors: FieldErrors::new(),
    }
    .into_response()
}

async fn setup(
    State(state): State<AppState>,
    form: Result<Form<SetupForm>, FormRejection>,
) -> Result<Response, AppError> {
    if !state.setup_pending() {
        return Ok(Redirect::to("/login").into_response());
    }
    let form = form_or_invalid(form)?;
    if !setup_token_matches(&state, &form.token) {
        return Err(AppError::Forbidden);
    }
    let input = match form.validate() {
        Ok(input) => input,
        Err(errors) => {
            return Ok(SetupTemplate {
                token: Some(form.token),
                username: form.username,
                errors,
            }
            .into_response());
        }
    };

    let password = input.password.clone();
    let hash = with_password_hashing(&state, move || hash_password(&password))
        .await?
        .map_err(|e| AppError::Internal(e.to_string().into()))?;
    match Admin::setup(&state.db, &input.username, &hash, &input.timezone).await {
        Ok(()) => {}
        // Another request finished the setup first.
        Err(e) if is_unique_violation(&e) => return Ok(Redirect::to("/login").into_response()),
        Err(e) => return Err(e.into()),
    }
    *state.setup_token.lock().unwrap() = None;
    tracing::info!("Admin account created; setup is complete.");

    start_session(&state).await
}

fn setup_token_matches(state: &AppState, token: &str) -> bool {
    state
        .setup_token
        .lock()
        .unwrap()
        .as_deref()
        .is_some_and(|expected| secrets_match(token, expected))
}

async fn login_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if state.setup_pending() {
        return Ok(Redirect::to("/setup").into_response());
    }
    if let Some(token) = cookie(&headers, SESSION_COOKIE)
        && Session::find(&state.db, token).await?.is_some()
    {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(LoginTemplate {
        username: String::new(),
        error: None,
    }
    .into_response())
}

async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    form: Result<Form<LoginForm>, FormRejection>,
) -> Result<Response, AppError> {
    if state.setup_pending() {
        return Ok(Redirect::to("/setup").into_response());
    }
    let form = form_or_invalid(form)?;
    if !state
        .login_limiter
        .lock()
        .unwrap()
        .try_attempt(ip, Instant::now())
    {
        return Err(AppError::TooManyRequests);
    }

    let Some(admin) = Admin::find(&state.db).await? else {
        // The admin row was removed while running: back to first-launch setup.
        if let Some(token) = state.start_setup_if_needed().await? {
            print_setup_url(&token);
        }
        return Ok(Redirect::to("/setup").into_response());
    };
    // The password is always checked and usernames are compared through their digests, so the
    // response time reveals nothing about the username.
    let password = form.password.clone();
    let hash = admin.password_hash.clone();
    let password_ok =
        with_password_hashing(&state, move || verify_password(&password, &hash)).await?;
    let username_ok = secrets_match(
        &sha256_hex(form.username.trim()),
        &sha256_hex(&admin.username),
    );

    if !(password_ok && username_ok) {
        tracing::warn!(%ip, "failed login attempt");
        return Ok(LoginTemplate {
            username: form.username,
            error: Some(LOGIN_FAILED),
        }
        .into_response());
    }
    state.login_limiter.lock().unwrap().record_success(ip);
    Session::purge_expired(&state.db).await?;
    start_session(&state).await
}

async fn logout(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, AppError> {
    session.delete(&state.db).await?;
    Ok((
        [(header::SET_COOKIE, expired_session_cookie())],
        Redirect::to("/login"),
    )
        .into_response())
}

async fn start_session(state: &AppState) -> Result<Response, AppError> {
    let session = Session::create(&state.db).await?;
    Ok((
        [(header::SET_COOKIE, session_cookie(&session.token))],
        Redirect::to("/"),
    )
        .into_response())
}

/// Runs an Argon2 computation on the blocking pool, one at a time. A request that cannot get
/// its turn within `HASHING_WAIT` is refused, so a flood cannot queue up without bound.
async fn with_password_hashing<T: Send + 'static>(
    state: &AppState,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    let _permit = tokio::time::timeout(HASHING_WAIT, state.password_hashing.acquire())
        .await
        .map_err(|_| AppError::TooManyRequests)?
        .map_err(|e| AppError::Internal(Box::new(e)))?;
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| AppError::Internal(Box::new(e)))
}

/// The client address: the socket peer, or the last `X-Forwarded-For` entry (the one added by
/// the reverse proxy) when `TRUST_PROXY_HEADERS` is set.
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let forwarded = state
            .trust_proxy_headers
            .then(|| forwarded_for(&parts.headers))
            .flatten();
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());
        Ok(Self(
            forwarded
                .or(peer)
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        ))
    }
}

fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get_all("x-forwarded-for")
        .iter()
        .next_back()?
        .to_str()
        .ok()?
        .rsplit(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn last_forwarded_entry_is_the_client() {
        let mut headers = HeaderMap::new();
        assert_eq!(forwarded_for(&headers), None);
        headers.append(
            "x-forwarded-for",
            HeaderValue::from_static("1.1.1.1, 2.2.2.2"),
        );
        headers.append(
            "x-forwarded-for",
            HeaderValue::from_static("3.3.3.3 , 4.4.4.4 "),
        );
        assert_eq!(forwarded_for(&headers), "4.4.4.4".parse().ok());
        headers.append("x-forwarded-for", HeaderValue::from_static("garbage"));
        assert_eq!(forwarded_for(&headers), None);
    }
}
