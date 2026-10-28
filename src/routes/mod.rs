use crate::error::AppError;
use crate::state::AppState;
use axum::Router;
use axum::extract::rejection::FormRejection;
use axum::extract::{Form, FromRequestParts, Path};
use axum::http::request::Parts;

mod health;
mod monitors;

pub fn all() -> Router<AppState> {
    Router::new()
        .merge(health::router())
        .merge(monitors::router())
}

/// Parses a positive integer id from a path segment; anything else is a 404.
pub fn parse_id(raw: &str) -> Result<i64, AppError> {
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or(AppError::NotFound)
}

/// A monitor id taken from the `{id}` path segment. Anything that is not a
/// positive integer, including a segment that is not valid UTF-8, is a 404.
pub struct MonitorId(pub i64);

impl<S: Send + Sync> FromRequestParts<S> for MonitorId {
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
