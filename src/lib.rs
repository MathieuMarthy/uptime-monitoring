use axum::Router;
use tower_http::trace::TraceLayer;

pub mod assets;
pub mod config;
pub mod db;
pub mod error;
pub mod models;
pub mod routes;
pub mod state;
pub mod time;
pub mod views;

use crate::error::AppError;
use crate::state::AppState;

pub fn app(state: AppState) -> Router {
    routes::all()
        .merge(assets::router())
        .fallback(|| async { AppError::NotFound })
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
