mod common;

use common::{TestApp, TestResponse, get, post_form};
use sqlx::SqlitePool;

const DEFAULTS: &str = "method=GET&accepted_statuses=200-299&interval_s=60&timeout_s=10\
    &failure_threshold=1&tls_expiry_warn_days=14&headers=&body=&keyword=\
    &follow_redirects=on&active=on";

fn form(name: &str, extra: &str) -> String {
    format!("name={name}&url=https%3A%2F%2Fexample.com&{DEFAULTS}{extra}")
}

async fn create(app: &TestApp, name: &str) -> TestResponse {
    post_form(app, "/monitors/new", &form(name, "")).await
}

async fn count_monitors(pool: &SqlitePool) -> i64 {
    sqlx::query_as::<_, (i64,)>("SELECT count(*) FROM monitors")
        .fetch_one(pool)
        .await
        .unwrap()
        .0
}

fn assert_redirect(r: &TestResponse, location: &str) {
    assert_eq!(r.status, 303, "body: {}", r.body);
    assert_eq!(r.headers["location"], location);
}

#[tokio::test]
async fn create_redirects_to_detail() {
    let (app, _) = common::test_app().await;
    assert_redirect(&create(&app, "API").await, "/monitors/1");
    let r = get(&app, "/monitors/1").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("API"));
}

#[tokio::test]
async fn invalid_create_shows_errors_and_inserts_nothing() {
    let (app, pool) = common::test_app().await;
    let r = post_form(&app, "/monitors/new", &form("", "")).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains(r#"class="field-error""#));
    assert_eq!(count_monitors(&pool).await, 0);
}

#[tokio::test]
async fn blank_number_field_shows_field_error() {
    let (app, pool) = common::test_app().await;
    for bad in ["", "abc"] {
        let body = form("API", "").replace("interval_s=60", &format!("interval_s={bad}"));
        let r = post_form(&app, "/monitors/new", &body).await;
        assert_eq!(r.status, 200, "interval_s={bad:?}");
        assert!(r.body.contains("Enter a whole number between 5 and 86400."));
    }
    assert_eq!(count_monitors(&pool).await, 0);
}

#[tokio::test]
async fn duplicate_name_is_field_error() {
    let (app, _) = common::test_app().await;
    create(&app, "API").await;
    let r = create(&app, "api").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("This name is already used."));

    assert_redirect(&create(&app, "Other").await, "/monitors/2");
    let r = post_form(&app, "/monitors/2/edit", &form("API", "")).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("This name is already used."));
}

#[tokio::test]
async fn edit_keeping_same_name_succeeds() {
    let (app, pool) = common::test_app().await;
    create(&app, "API").await;
    let body = form("API", "").replace("interval_s=60", "interval_s=120");
    assert_redirect(
        &post_form(&app, "/monitors/1/edit", &body).await,
        "/monitors/1",
    );
    let (interval,): (i64,) = sqlx::query_as("SELECT interval_s FROM monitors WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(interval, 120);
}

#[tokio::test]
async fn edit_form_is_prefilled() {
    let (app, _) = common::test_app().await;
    create(&app, "API").await;
    let r = get(&app, "/monitors/1/edit").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains(r#"value="API""#));
    assert!(r.body.contains(r#"value="https://example.com""#));
    assert_eq!(
        r.body.matches("checked/>").count(),
        2,
        "both boxes checked by default"
    );
}

#[tokio::test]
async fn invalid_form_keeps_user_values() {
    let (app, _) = common::test_app().await;
    let body = "name=My+API&url=x&method=GET&accepted_statuses=200-299&interval_s=60\
        &timeout_s=10&failure_threshold=1&tls_expiry_warn_days=14";
    let r = post_form(&app, "/monitors/new", body).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains(r#"value="My API""#));
    assert!(r.body.contains(r#"value="x""#));
    assert!(
        !r.body.contains("checked/>"),
        "unchecked boxes must stay unchecked"
    );
}

#[tokio::test]
async fn toggle_pause_and_delete() {
    let (app, pool) = common::test_app().await;
    create(&app, "API").await;
    assert_redirect(
        &post_form(&app, "/monitors/1/toggle-pause", "").await,
        "/monitors/1",
    );
    let (paused,): (bool,) = sqlx::query_as("SELECT paused FROM monitors WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(paused);
    assert_redirect(&post_form(&app, "/monitors/1/delete", "").await, "/");
    assert_eq!(get(&app, "/monitors/1").await.status, 404);
}

#[tokio::test]
async fn bad_ids_return_404() {
    let (app, _) = common::test_app().await;
    create(&app, "API").await;
    for id in ["abc", "-1", "0", "999", "1.5", "+1", "%2B1"] {
        let responses = [
            get(&app, &format!("/monitors/{id}")).await,
            get(&app, &format!("/monitors/{id}/edit")).await,
            post_form(&app, &format!("/monitors/{id}/edit"), &form("API", "")).await,
            post_form(&app, &format!("/monitors/{id}/delete"), "").await,
            post_form(&app, &format!("/monitors/{id}/toggle-pause"), "").await,
        ];
        for r in responses {
            assert_eq!(r.status, 404, "id {id}");
            assert!(r.body.contains("Page not found"), "id {id}");
        }
    }
}

#[tokio::test]
async fn dashboard_counts_states() {
    let (app, pool) = common::test_app().await;
    for name in ["a", "b", "c"] {
        create(&app, name).await;
    }
    post_form(&app, "/monitors/1/toggle-pause", "").await;
    sqlx::query("UPDATE monitor_state SET status = 'down' WHERE monitor_id = 2")
        .execute(&pool)
        .await
        .unwrap();
    let r = get(&app, "/").await;
    assert_eq!(r.status, 200);
    for (label, count) in [("up", 0), ("down", 1), ("pending", 1), ("paused", 1)] {
        let needle = format!(r#"data-count="{label}">{count}<"#);
        assert!(r.body.contains(&needle), "missing {needle}");
    }
}

#[tokio::test]
async fn pages_have_no_service_wording() {
    let (app, _) = common::test_app().await;
    create(&app, "API").await;
    for uri in ["/", "/monitors/new", "/monitors/1", "/monitors/1/edit"] {
        let body = get(&app, uri).await.body.to_lowercase();
        assert!(!body.contains("service"), "{uri} mentions 'service'");
    }
}

#[tokio::test]
async fn malformed_form_bodies_render_html_error_page() {
    use axum::body::Body;
    use axum::http::Request;

    let (app, pool) = common::test_app().await;
    let duplicate_field = post_form(&app, "/monitors/new", &form("API", "&name=Other")).await;
    let duplicate_checkbox = post_form(&app, "/monitors/new", &form("API", "&active=on")).await;
    let session = app.session.clone().unwrap();
    let wrong_type = common::send(
        &app,
        Request::post("/monitors/new")
            .header("content-type", "text/plain")
            .header("cookie", format!("__Host-session={}", session.token))
            .body(Body::from(form(
                "API",
                &format!("&csrf_token={}", session.csrf_token),
            )))
            .unwrap(),
    )
    .await;
    let too_large = post_form(
        &app,
        "/monitors/new",
        &form("API", &format!("&body={}", "a".repeat(200 * 1024))),
    )
    .await;

    for r in [duplicate_field, duplicate_checkbox, wrong_type, too_large] {
        assert!(r.status.is_client_error(), "status {}", r.status);
        assert!(r.body.contains("Invalid request"), "body: {}", r.body);
        assert!(
            !r.body.contains("Failed to"),
            "raw rejection leaked: {}",
            r.body
        );
        assert!(
            !r.body.contains("Content-Type"),
            "raw rejection leaked: {}",
            r.body
        );
    }
    assert_eq!(count_monitors(&pool).await, 0);
}

#[tokio::test]
async fn malformed_edit_on_unknown_id_is_404() {
    let (app, _) = common::test_app().await;
    let r = post_form(&app, "/monitors/999/edit", &form("API", "&name=Other")).await;
    assert_eq!(r.status, 404);
    assert!(r.body.contains("Page not found"));
}

#[tokio::test]
async fn non_utf8_path_segments_return_404_page() {
    let (app, _) = common::test_app().await;
    let responses = [
        get(&app, "/monitors/%FF").await,
        get(&app, "/monitors/%FF/edit").await,
        post_form(&app, "/monitors/%FF/edit", &form("API", "")).await,
        post_form(&app, "/monitors/%FF/delete", "").await,
        post_form(&app, "/monitors/%FF/toggle-pause", "").await,
        get(&app, "/static/%FF").await,
    ];
    for r in responses {
        assert_eq!(r.status, 404, "body: {}", r.body);
        assert!(r.body.contains("Page not found"));
    }
}

#[tokio::test]
async fn invisible_name_is_field_error() {
    let (app, pool) = common::test_app().await;
    for name in ["%E2%80%8B", "%C2%A0%E2%80%8D", "%EF%BB%BF+"] {
        let r = create(&app, name).await;
        assert_eq!(r.status, 200, "name {name}");
        assert!(r.body.contains("Name is required."), "name {name}");
    }
    assert_eq!(count_monitors(&pool).await, 0);
}

#[tokio::test]
async fn wrong_method_renders_405_page() {
    let (app, _) = common::test_app().await;
    create(&app, "API").await;
    let r = get(&app, "/monitors/1/delete").await;
    assert_eq!(r.status, 405);
    assert!(r.body.contains("Method not allowed"));
}
