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
pub mod privileges;
pub mod routes;
pub mod state;
pub mod time;
pub mod transfer;
pub mod views;
pub mod windows;

use crate::error::AppError;
use crate::state::AppState;

/// Largest legitimate form: a 64 KiB request body plus the other fields.
pub const MAX_BODY_BYTES: usize = 128 * 1024;

/// Version shown in the admin footer and sent with bug reports.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Source repository, linked from the admin footer.
pub const REPO_URL: &str = "https://github.com/MathieuMarthy/uptime-monitoring";

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
