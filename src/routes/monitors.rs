use crate::models::{Monitor, ServiceStatus};
use crate::state::AppState;
use crate::templates::monitors::MonitorIndexTemplate;
use axum::extract::State;
use axum::routing::get;
use axum::Router;

async fn index(State(state): State<AppState>) -> MonitorIndexTemplate {
    let monitors = Monitor::fetch_all(&state.db).await.unwrap_or_default();

    let up_count = monitors
        .iter()
        .filter(|m| m.service.is_enabled && m.service.current_status == ServiceStatus::Up)
        .count() as i64;
    let down_count = monitors
        .iter()
        .filter(|m| m.service.is_enabled && m.service.current_status == ServiceStatus::Down)
        .count() as i64;
    let paused_count = monitors
        .iter()
        .filter(|m| !m.service.is_enabled)
        .count() as i64;

    MonitorIndexTemplate {
        monitors,
        up_count,
        down_count,
        paused_count,
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(index))
}
