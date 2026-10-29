use axum::Router;
use axum::extract::DefaultBodyLimit;
use tower_http::trace::TraceLayer;

pub mod assets;
pub mod auth;
pub mod config;
pub mod db;
pub mod engine;
pub mod error;
pub mod forms;
pub mod healthcheck;
pub mod maintenance;
pub mod middleware;
pub mod models;
pub mod notify;
pub mod page_cache;
pub mod routes;
pub mod state;
pub mod time;
pub mod views;

use crate::error::AppError;
use crate::state::AppState;

/// Largest legitimate form: a 64 KiB request body plus the other fields.
pub const MAX_BODY_BYTES: usize = 128 * 1024;

pub fn app(state: AppState) -> Router {
    routes::all(&state)
        .merge(assets::router())
        .fallback(|| async { AppError::NotFound })
        .method_not_allowed_fallback(|| async { AppError::MethodNotAllowed })
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(axum::middleware::map_response(middleware::security_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
