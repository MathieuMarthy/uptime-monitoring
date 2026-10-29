mod common;

use std::net::SocketAddr;
use uptime_monitoring::healthcheck::{probe, target};

#[test]
fn unspecified_bind_address_is_probed_on_loopback() {
    let probe_of = |bind: &str| target(bind.parse::<SocketAddr>().unwrap()).to_string();
    assert_eq!(probe_of("0.0.0.0:8080"), "127.0.0.1:8080");
    assert_eq!(probe_of("[::]:9000"), "[::1]:9000");
    assert_eq!(probe_of("10.0.0.5:8080"), "10.0.0.5:8080");
}

#[tokio::test]
async fn probe_succeeds_against_a_healthy_instance() {
    let (app, _) = common::test_app().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.router).await.unwrap() });
    probe(addr).await.unwrap();
}

#[tokio::test]
async fn probe_fails_when_nothing_listens_or_health_is_not_ok() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed = listener.local_addr().unwrap();
    drop(listener);
    assert!(probe(closed).await.is_err());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let unhealthy = axum::Router::new().route(
        "/health",
        axum::routing::get(|| async { (axum::http::StatusCode::SERVICE_UNAVAILABLE, "no") }),
    );
    tokio::spawn(async move { axum::serve(listener, unhealthy).await.unwrap() });
    let error = probe(addr).await.unwrap_err();
    assert!(error.contains("503"), "{error}");
}
