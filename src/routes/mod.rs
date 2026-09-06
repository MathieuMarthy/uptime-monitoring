use axum::Router;

mod health;
mod monitors;

pub fn all() -> Router {
    Router::new()
        .merge(health::router())
        .merge(monitors::router())
}
