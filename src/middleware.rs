//! Request guards shared by every route: session and CSRF checks, security headers.

use crate::auth::{SESSION_COOKIE, cookie, secrets_match};
use crate::error::AppError;
use crate::i18n::{self, Lang};
use crate::models::Session;
use crate::state::AppState;
use crate::{IMPORT_MAX_BODY_BYTES, IMPORT_PATH, MAX_BODY_BYTES};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
    img-src 'self' data:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; \
    form-action 'self'";

/// Lets a request through only with a valid session, and a POST only with its CSRF token.
/// The session is handed to handlers as an extension; responses are never cached.
pub async fn require_session(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    if state.setup_pending() {
        return Ok(Redirect::to("/setup").into_response());
    }
    let session = match cookie(request.headers(), SESSION_COOKIE) {
        Some(token) => Session::find(&state.db, token).await?,
        None => None,
    };
    let Some(session) = session else {
        return Ok(Redirect::to("/login").into_response());
    };

    let mut request = if request.method() == Method::GET || request.method() == Method::HEAD {
        request
    } else {
        check_csrf(request, &session.csrf_token).await?
    };
    request.extensions_mut().insert(session);

    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// Reads the urlencoded body, checks its `csrf_token` field and rebuilds the request.
async fn check_csrf(request: Request, expected: &str) -> Result<Request, AppError> {
    let is_form = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/x-www-form-urlencoded"));
    if !is_form {
        return Err(AppError::InvalidRequest(StatusCode::UNSUPPORTED_MEDIA_TYPE));
    }
    let limit = if request.uri().path() == IMPORT_PATH {
        IMPORT_MAX_BODY_BYTES
    } else {
        MAX_BODY_BYTES
    };
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, limit)
        .await
        .map_err(|_| AppError::InvalidRequest(StatusCode::PAYLOAD_TOO_LARGE))?;
    let valid = form_urlencoded::parse(&bytes)
        .find(|(key, _)| key == "csrf_token")
        .is_some_and(|(_, token)| secrets_match(&token, expected));
    if !valid {
        return Err(AppError::Forbidden);
    }
    Ok(Request::from_parts(parts, Body::from(bytes)))
}

pub async fn security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "same-origin"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}

/// Runs the request in its language: the instance one, or before setup the `lang` query
/// parameter, else the browser's `Accept-Language`.
pub async fn set_language(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let lang = if state.setup_pending() {
        let picked = request.uri().query().and_then(|query| {
            form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == "lang")
                .and_then(|(_, value)| Lang::from_code(&value))
        });
        picked
            .or_else(|| {
                request
                    .headers()
                    .get(header::ACCEPT_LANGUAGE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(Lang::from_accept_language)
            })
            .unwrap_or_default()
    } else {
        state.language.get()
    };
    i18n::scope(lang, next.run(request)).await
}
