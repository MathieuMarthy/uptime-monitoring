use crate::state::AppState;
use axum::Router;

mod health;
mod monitors;

pub fn all() -> Router<AppState> {
    Router::new()
        .merge(health::router())
        .merge(monitors::router())
}
