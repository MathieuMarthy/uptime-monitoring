#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tower::ServiceExt;
use uptime_monitoring::state::AppState;

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

pub async fn test_app() -> (Router, SqlitePool) {
    let pool = uptime_monitoring::db::init_pool("sqlite::memory:")
        .await
        .expect("in-memory pool");
    let app = uptime_monitoring::app(AppState::new(pool.clone()));
    (app, pool)
}

pub async fn send(app: &Router, request: Request<Body>) -> TestResponse {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    TestResponse {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

pub async fn get(app: &Router, uri: &str) -> TestResponse {
    send(app, Request::get(uri).body(Body::empty()).unwrap()).await
}

pub async fn post_form(app: &Router, uri: &str, body: &str) -> TestResponse {
    let request = Request::post(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    send(app, request).await
}
