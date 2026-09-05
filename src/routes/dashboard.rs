use crate::templates::dashboard::DashboardIndexTemplate;
use axum::Router;
use axum::routing::get;

async fn index() -> DashboardIndexTemplate {
    DashboardIndexTemplate {}
}

pub fn router() -> Router {
    Router::new().route("/", get(index))
}
