use crate::templates::monitors::MonitorIndexTemplate;
use axum::Router;
use axum::routing::get;

async fn index() -> MonitorIndexTemplate {
    MonitorIndexTemplate {}
}

pub fn router() -> Router {
    Router::new().route("/", get(index))
}
