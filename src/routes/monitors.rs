use crate::models::{Incident, Service, ServiceFormInput, ServiceStatus};
use crate::views::monitor_view::Monitor;
use crate::state::AppState;
use crate::views::monitors::{
    MonitorDetailTemplate, MonitorFormTemplate, MonitorIndexTemplate,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};

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

async fn new_form() -> MonitorFormTemplate {
    MonitorFormTemplate {
        service: None,
        action_url: "/monitors/new".to_string(),
        cancel_url: "/".to_string(),
        is_edit: false,
        error_message: None,
    }
}

async fn create(
    State(state): State<AppState>,
    Form(input): Form<ServiceFormInput>,
) -> Response {
    if let Err(err) = input.validate() {
        let dummy = input.to_dummy_service(None);
        return MonitorFormTemplate {
            service: Some(dummy),
            action_url: "/monitors/new".to_string(),
            cancel_url: "/".to_string(),
            is_edit: false,
            error_message: Some(err),
        }
        .into_response();
    }

    match Service::create(&state.db, input.to_create_service()).await {
        Ok(service) => Redirect::to(&format!("/monitors/{}", service.id)).into_response(),
        Err(e) => {
            let dummy = input.to_dummy_service(None);
            MonitorFormTemplate {
                service: Some(dummy),
                action_url: "/monitors/new".to_string(),
                cancel_url: "/".to_string(),
                is_edit: false,
                error_message: Some(format!("Database error: {}", e)),
            }
            .into_response()
        }
    }
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let monitor = match Monitor::fetch_by_id(&state.db, &id).await {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "Monitor not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let incidents = Incident::find_recent_by_service(&state.db, &id, 10)
        .await
        .unwrap_or_default();

    MonitorDetailTemplate { monitor, incidents }.into_response()
}

async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let service = match Service::find_by_id(&state.db, &id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "Monitor not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    MonitorFormTemplate {
        service: Some(service),
        action_url: format!("/monitors/{}/edit", id),
        cancel_url: format!("/monitors/{}", id),
        is_edit: true,
        error_message: None,
    }
    .into_response()
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(input): Form<ServiceFormInput>,
) -> Response {
    if let Err(err) = input.validate() {
        let dummy = input.to_dummy_service(Some(id.clone()));
        return MonitorFormTemplate {
            service: Some(dummy),
            action_url: format!("/monitors/{}/edit", id),
            cancel_url: format!("/monitors/{}", id),
            is_edit: true,
            error_message: Some(err),
        }
        .into_response();
    }

    match Service::update(&state.db, &id, input.to_edit_service()).await {
        Ok(Some(_)) => Redirect::to(&format!("/monitors/{}", id)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Monitor not found").into_response(),
        Err(e) => {
            let dummy = input.to_dummy_service(Some(id.clone()));
            MonitorFormTemplate {
                service: Some(dummy),
                action_url: format!("/monitors/{}/edit", id),
                cancel_url: format!("/monitors/{}", id),
                is_edit: true,
                error_message: Some(format!("Failed to update: {}", e)),
            }
            .into_response()
        }
    }
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match Service::delete(&state.db, &id).await {
        Ok(_) => Redirect::to("/").into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to delete monitor: {}", e),
        )
            .into_response(),
    }
}

async fn toggle_pause(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let service = match Service::find_by_id(&state.db, &id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "Monitor not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let new_state = !service.is_enabled;
    match Service::set_enabled(&state.db, &id, new_state).await {
        Ok(_) => Redirect::to(&format!("/monitors/{}", id)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/monitors/new", get(new_form).post(create))
        .route("/monitors/{id}", get(detail))
        .route("/monitors/{id}/edit", get(edit_form).post(update))
        .route("/monitors/{id}/delete", post(delete))
        .route("/monitors/{id}/toggle-pause", post(toggle_pause))
}
