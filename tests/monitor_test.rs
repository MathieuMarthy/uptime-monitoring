//! The "Test" button of the monitor form: one check with the values typed, nothing saved.

mod common;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use common::{TestApp, post_form};
use sqlx::SqlitePool;

/// A local target: `/ok` answers 200, `/broken` 500.
async fn target() -> String {
    let app = Router::new().route("/ok", get(|| async { "ok" })).route(
        "/broken",
        get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "no") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http%3A%2F%2F{addr}")
}

fn form(name: &str, url: &str) -> String {
    format!(
        "name={name}&kind=http&url={url}&method=GET&accepted_statuses=200-299&interval_s=60\
         &retry_interval_s=20&timeout_s=5&failure_threshold=1&tls_expiry_warn_days=14\
         &follow_redirects=on&active=on"
    )
}

async fn count_monitors(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM monitors")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn test(app: &TestApp, uri: &str, body: &str) -> String {
    let r = post_form(app, uri, body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.body
}

#[tokio::test]
async fn passing_test_shows_the_response_and_saves_nothing() {
    let (app, pool) = common::test_app().await;
    let url = format!("{}%2Fok", target().await);
    let body = test(&app, "/monitors/new/test", &form("API", &url)).await;
    assert!(body.contains("Test passed: HTTP 200 in "), "{body}");
    assert!(body.contains("The monitor is not saved yet."));
    assert!(
        body.contains(r#"value="API""#),
        "the form keeps what was typed"
    );
    assert_eq!(count_monitors(&pool).await, 0);
}

#[tokio::test]
async fn failing_test_gives_the_reason() {
    let (app, _) = common::test_app().await;
    let url = format!("{}%2Fbroken", target().await);
    let body = test(&app, "/monitors/new/test", &form("API", &url)).await;
    assert!(body.contains("Test failed: HTTP 500"), "{body}");
}

#[tokio::test]
async fn invalid_form_shows_its_errors_and_runs_nothing() {
    let (app, _) = common::test_app().await;
    let body = test(&app, "/monitors/new/test", &form("", "nope")).await;
    assert!(body.contains(r#"class="field-error""#));
    assert!(!body.contains("Test passed") && !body.contains("Test failed"));
}

#[tokio::test]
async fn edit_form_tests_without_saving() {
    let (app, pool) = common::test_app().await;
    let url = format!("{}%2Fok", target().await);
    let r = post_form(&app, "/monitors/new", &form("API", &url)).await;
    assert_eq!(r.status, 303);

    let broken = format!("{}%2Fbroken", target().await);
    let body = test(&app, "/monitors/1/edit/test", &form("API", &broken)).await;
    assert!(body.contains("Test failed: HTTP 500"), "{body}");
    assert!(body.contains(r#"action="/monitors/1/edit""#));
    let body = test(&app, "/monitors/1/edit/test", &form("API", &url)).await;
    assert!(body.contains("Changes are not saved yet."), "{body}");
    let saved: String = sqlx::query_scalar("SELECT url FROM monitors WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(saved.ends_with("/ok"));

    let r = post_form(&app, "/monitors/9/edit/test", &form("API", &url)).await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn tests_wait_their_turn() {
    let (app, _) = common::test_app().await;
    let _all = app
        .state
        .monitor_tests
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let url = format!("{}%2Fok", target().await);
    let body = test(&app, "/monitors/new/test", &form("API", &url)).await;
    assert!(body.contains("Other tests are running"), "{body}");
}

#[tokio::test]
async fn form_has_a_test_button() {
    let (app, _) = common::test_app().await;
    let r = common::get(&app, "/monitors/new").await;
    assert!(r.body.contains(r#"formaction="/monitors/new/test""#));
}

#[tokio::test]
async fn push_monitor_has_nothing_to_test() {
    let (app, _) = common::test_app().await;
    let push = "name=backup&kind=push&interval_s=60&grace_s=60&failure_threshold=1&active=on";
    let body = test(&app, "/monitors/new/test", push).await;
    assert!(body.contains("A push monitor waits for its job"), "{body}");
}
