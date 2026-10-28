use crate::error::AppError;
use crate::state::AppState;
use axum::Router;

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
