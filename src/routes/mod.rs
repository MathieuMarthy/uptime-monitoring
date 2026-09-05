use axum::Router;

mod dashboard;
mod health;

pub fn all() -> Router {
    Router::new()
        .merge(health::router())
        .merge(dashboard::router())
}
