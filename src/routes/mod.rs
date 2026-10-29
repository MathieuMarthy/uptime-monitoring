use crate::error::AppError;
use crate::state::AppState;
use axum::extract::rejection::FormRejection;
use axum::extract::{Form, FromRequestParts, Path};
use axum::http::request::Parts;
use axum::{Router, middleware};

mod alerts;
mod auth;
mod health;
mod monitors;
mod status_pages;

/// Every route requires a session unless it is listed in the public router.
pub fn all(state: &AppState) -> Router<AppState> {
    let protected = Router::new()
        .merge(monitors::router())
        .merge(alerts::router())
        .merge(status_pages::router())
        .merge(auth::protected_router())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::require_session,
        ));
    let public = Router::new()
        .merge(health::router())
        .merge(auth::public_router())
        .merge(status_pages::public_router());
    protected.merge(public)
}

/// Parses a positive integer id from a path segment; anything else is a 404.
pub fn parse_id(raw: &str) -> Result<i64, AppError> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::NotFound);
    }
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or(AppError::NotFound)
}

/// An id taken from the `{id}` path segment. Anything that is not a
/// positive integer, including a segment that is not valid UTF-8, is a 404.
pub struct PathId(pub i64);

impl<S: Send + Sync> FromRequestParts<S> for PathId {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(raw) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::NotFound)?;
        parse_id(&raw).map(Self)
    }
}

/// Turns a form extraction failure into the HTML error page instead of Axum's plain-text rejection.
pub fn form_or_invalid<T>(form: Result<Form<T>, FormRejection>) -> Result<T, AppError> {
    form.map(|Form(value)| value)
        .map_err(|rejection| AppError::InvalidRequest(rejection.status()))
}
