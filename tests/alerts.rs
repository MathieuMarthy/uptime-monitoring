mod common;

use axum::Router;
use axum::routing::post;
use common::{TestResponse, get, post_form};
use sqlx::SqlitePool;
use uptime_monitoring::models::Channel;

fn assert_redirect(r: &TestResponse, location: &str) {
    assert_eq!(r.status, 303, "body: {}", r.body);
    assert_eq!(r.headers["location"], location);
}

fn encode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn webhook(name: &str, url: &str) -> String {
    format!("name={name}&kind=webhook&url={}&enabled=on", encode(url))
}

fn ntfy(name: &str, token: &str) -> String {
    ntfy_at(name, "https://ntfy.sh", token)
}

fn ntfy_at(name: &str, server: &str, token: &str) -> String {
    format!(
        "name={name}&kind=ntfy&ntfy_server={}&ntfy_topic=alerts&ntfy_token={token}&enabled=on",
        encode(server)
    )
}

async fn channels(pool: &SqlitePool) -> Vec<Channel> {
    Channel::all(pool).await.unwrap()
}

#[tokio::test]
async fn alerts_page_lists_channels() {
    let (app, _) = common::test_app().await;
    let r = get(&app, "/alerts").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("No notification channel yet"));

    post_form(
        &app,
        "/alerts/new",
        &webhook("Ops+hook", "https://hooks.example.com/x"),
    )
    .await;
    let r = get(&app, "/alerts").await;
    assert!(r.body.contains("Ops hook"));
    assert!(r.body.contains("webhook"));
}

#[tokio::test]
async fn create_validates_and_stores_the_channel() {
    let (app, pool) = common::test_app().await;
    let r = post_form(&app, "/alerts/new", &webhook("Ops", "not a url")).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains(r#"class="field-error""#));
    assert!(channels(&pool).await.is_empty());

    let r = post_form(
        &app,
        "/alerts/new",
        &webhook("Ops", "https://hooks.example.com/x"),
    )
    .await;
    assert_redirect(&r, "/alerts");
    let stored = channels(&pool).await;
    assert_eq!(
        (stored[0].name.as_str(), stored[0].kind.as_str()),
        ("Ops", "webhook")
    );
    assert!(stored[0].enabled);

    let r = post_form(
        &app,
        "/alerts/new",
        &webhook("OPS", "https://hooks.example.com/y"),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("This name is already used."));
}

#[tokio::test]
async fn edit_never_shows_the_ntfy_token_and_keeps_it() {
    let (app, pool) = common::test_app().await;
    post_form(&app, "/alerts/new", &ntfy("Phone", "tk_secret")).await;
    let id = channels(&pool).await[0].id;

    let r = get(&app, &format!("/alerts/{id}/edit")).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("alerts"), "topic prefilled");
    assert!(!r.body.contains("tk_secret"));

    let r = post_form(&app, &format!("/alerts/{id}/edit"), &ntfy("Phone2", "")).await;
    assert_redirect(&r, "/alerts");
    let stored = &channels(&pool).await[0];
    assert_eq!(stored.name, "Phone2");
    assert!(stored.config.contains("tk_secret"));
}

#[tokio::test]
async fn delete_removes_the_channel() {
    let (app, pool) = common::test_app().await;
    post_form(
        &app,
        "/alerts/new",
        &webhook("Ops", "https://hooks.example.com/x"),
    )
    .await;
    let id = channels(&pool).await[0].id;
    assert_redirect(
        &post_form(&app, &format!("/alerts/{id}/delete"), "").await,
        "/alerts",
    );
    assert!(channels(&pool).await.is_empty());
    assert_eq!(
        post_form(&app, &format!("/alerts/{id}/delete"), "")
            .await
            .status,
        404
    );
}

async fn hook() -> String {
    let hook = Router::new().route("/hook", post(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, hook).await.unwrap() });
    url
}

#[tokio::test]
async fn form_tests_the_unsaved_channel() {
    let (app, pool) = common::test_app().await;
    let url = hook().await;
    let r = post_form(&app, "/alerts/new/test", &webhook("Ops", &url)).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Test message sent."), "{}", r.body);
    assert!(r.body.contains(&format!(r#"value="{url}""#)), "values kept");
    assert!(channels(&pool).await.is_empty(), "nothing saved");

    let r = post_form(
        &app,
        "/alerts/new/test",
        &webhook("Ops", "http://127.0.0.1:9/hook"),
    )
    .await;
    assert!(r.body.contains("Test failed:"), "{}", r.body);

    let r = post_form(&app, "/alerts/new/test", &webhook("Ops", "nope")).await;
    assert!(r.body.contains(r#"class="field-error""#));
}

#[tokio::test]
async fn edit_form_test_uses_the_stored_token_and_keeps_a_typed_one() {
    let (app, pool) = common::test_app().await;
    let server = hook().await;
    post_form(&app, "/alerts/new", &ntfy_at("Phone", &server, "tk_secret")).await;
    let id = channels(&pool).await[0].id;
    let r = post_form(
        &app,
        &format!("/alerts/{id}/edit/test"),
        &ntfy_at("Phone", &server, ""),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Test message sent."), "{}", r.body);
    assert!(
        !r.body.contains("tk_secret"),
        "the stored token stays hidden"
    );

    let r = post_form(
        &app,
        &format!("/alerts/{id}/edit/test"),
        &ntfy_at("Phone", &server, "tk_typed"),
    )
    .await;
    assert!(
        r.body.contains(r#"value="tk_typed""#),
        "the typed token survives"
    );
    assert_eq!(
        post_form(&app, "/alerts/77/edit/test", &ntfy("P", ""))
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn unknown_channels_are_404() {
    let (app, _) = common::test_app().await;
    for uri in ["/alerts/7/edit", "/alerts/x/edit", "/alerts/0/edit"] {
        assert_eq!(get(&app, uri).await.status, 404, "{uri}");
    }
    let body = webhook("Ops", "https://hooks.example.com/x");
    assert_eq!(post_form(&app, "/alerts/7/edit", &body).await.status, 404);
}

#[tokio::test]
async fn alerts_need_a_session() {
    let (app, _) = common::test_app().await;
    let r = get(&app.logged_out(), "/alerts").await;
    assert_eq!(r.status, 303);
}
