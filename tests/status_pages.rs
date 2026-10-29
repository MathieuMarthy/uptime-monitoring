mod common;

use common::{TestApp, TestResponse, get, post_form};
use sqlx::SqlitePool;
use uptime_monitoring::forms::monitor::MonitorForm;
use uptime_monitoring::models::{Monitor, StatusPage};
use uptime_monitoring::time::now_ms;

const SECRET_URL: &str = "https://internal.example.net/secret-path";

fn assert_redirect(r: &TestResponse, location: &str) {
    assert_eq!(r.status, 303, "body: {}", r.body);
    assert_eq!(r.headers["location"], location);
}

async fn monitor(pool: &SqlitePool, name: &str) -> i64 {
    let input = MonitorForm {
        name: name.to_string(),
        url: SECRET_URL.to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap();
    Monitor::create(pool, &input).await.unwrap()
}

/// A published page `status` showing `monitors`, in this order, without display names.
async fn create_page(app: &TestApp, monitors: &[i64]) {
    let mut body = "title=Our+Status&slug=status&description=Live+status&published=on".to_string();
    for (i, id) in monitors.iter().enumerate() {
        body.push_str(&format!("&monitor_{id}=on&position_{id}={}", i + 1));
    }
    let r = post_form(app, "/public-pages/new", &body).await;
    assert_redirect(&r, "/public-pages");
}

async fn page_id(pool: &SqlitePool) -> i64 {
    StatusPage::find_by_slug(pool, "status")
        .await
        .unwrap()
        .expect("page")
        .id
}

#[tokio::test]
async fn admin_pages_need_a_session() {
    let (app, pool) = common::test_app().await;
    let r = get(&app.logged_out(), "/public-pages").await;
    assert_redirect(&r, "/login");
    let r = post_form(&app.logged_out(), "/public-pages/new", "title=x&slug=x").await;
    assert_redirect(&r, "/login");
    assert!(StatusPage::summaries(&pool).await.unwrap().is_empty());
    let r = get(&app.logged_out(), "/public-pages/1/preview").await;
    assert_redirect(&r, "/login");
}

#[tokio::test]
async fn create_validates_lists_and_rejects_duplicate_slugs() {
    let (app, pool) = common::test_app().await;
    let r = get(&app, "/public-pages").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("No public page yet"));

    let r = post_form(&app, "/public-pages/new", "title=Status&slug=Bad+Slug").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains(r#"class="field-error""#));
    assert!(StatusPage::summaries(&pool).await.unwrap().is_empty());

    let id = monitor(&pool, "API").await;
    create_page(&app, &[id]).await;
    let r = get(&app, "/public-pages").await;
    assert!(r.body.contains("Our Status"));
    assert!(r.body.contains(r#"href="/status/status""#));

    let r = post_form(&app, "/public-pages/new", "title=Other&slug=status").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("This slug is already used."));
}

#[tokio::test]
async fn public_page_shows_selected_monitors_in_order() {
    let (app, pool) = common::test_app().await;
    let api = monitor(&pool, "API").await;
    let web = monitor(&pool, "Website").await;
    let hidden = monitor(&pool, "Hidden thing").await;
    let body = format!(
        "title=Our+Status&slug=status&published=on&monitor_{api}=on&position_{api}=2\
         &display_name_{api}=Public+API&monitor_{web}=on&position_{web}=1"
    );
    assert_redirect(
        &post_form(&app, "/public-pages/new", &body).await,
        "/public-pages",
    );
    sqlx::query(
        "UPDATE monitor_state SET status = 'down', last_error = 'connect 10.0.0.5 refused'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO incidents (monitor_id, started_at, error_kind, cause)
         VALUES (?, ?, 'connect', 'connect 10.0.0.5 refused')",
    )
    .bind(api)
    .bind(now_ms() - 3_600_000)
    .execute(&pool)
    .await
    .unwrap();

    let r = get(&app.logged_out(), "/status/status").await;
    assert_eq!(r.status, 200, "body: {}", r.body);
    assert!(r.body.contains("Our Status"));
    let (website, public_api) = (
        r.body.find("Website").expect("website"),
        r.body.find("Public API").expect("display name"),
    );
    assert!(website < public_api, "position order");
    assert!(
        !r.body.contains(">API<"),
        "display name replaces the monitor name"
    );
    assert!(!r.body.contains("Hidden thing"));
    assert!(
        !r.body.contains("internal.example.net"),
        "URLs are never shown"
    );
    assert!(!r.body.contains("10.0.0.5"), "errors are never shown");
    assert!(r.body.contains("Some systems are down"));
    assert!(r.body.contains("Ongoing"), "the open incident is listed");
    assert_eq!(r.headers["cache-control"], "no-cache");
    let _ = hidden;
}

#[tokio::test]
async fn unpublished_and_unknown_pages_are_not_found() {
    let (app, pool) = common::test_app().await;
    let r = post_form(&app, "/public-pages/new", "title=Draft&slug=status").await;
    assert_redirect(&r, "/public-pages");
    let id = page_id(&pool).await;

    let r = get(&app.logged_out(), "/status/status").await;
    assert_eq!(r.status, 404);
    let r = get(&app, "/status/status").await;
    assert_eq!(r.status, 404, "even for the admin");
    let r = get(&app.logged_out(), "/status/nothing-here").await;
    assert_eq!(r.status, 404);

    let r = get(&app, &format!("/public-pages/{id}/preview")).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Draft"));
    assert!(r.body.contains("not published"));
}

#[tokio::test]
async fn public_page_is_cached_until_the_page_changes() {
    let (app, pool) = common::test_app().await;
    let api = monitor(&pool, "API").await;
    create_page(&app, &[api]).await;
    let id = page_id(&pool).await;
    let r = get(&app.logged_out(), "/status/status").await;
    assert!(r.body.contains("API"));
    assert!(r.body.contains("All systems operational"));

    sqlx::query("UPDATE monitors SET name = 'Renamed'")
        .execute(&pool)
        .await
        .unwrap();
    let r = get(&app.logged_out(), "/status/status").await;
    assert!(!r.body.contains("Renamed"), "served from the cache");
    let r = get(&app, &format!("/public-pages/{id}/preview")).await;
    assert!(r.body.contains("Renamed"), "the preview is never cached");

    let body = format!("title=New+Title&slug=status&published=on&monitor_{api}=on");
    let r = post_form(&app, &format!("/public-pages/{id}/edit"), &body).await;
    assert_redirect(&r, "/public-pages");
    let r = get(&app.logged_out(), "/status/status").await;
    assert!(r.body.contains("New Title"));
    assert!(r.body.contains("Renamed"));
}

#[tokio::test]
async fn edit_form_shows_the_stored_page() {
    let (app, pool) = common::test_app().await;
    let api = monitor(&pool, "API").await;
    let web = monitor(&pool, "Website").await;
    create_page(&app, &[web, api]).await;
    let id = page_id(&pool).await;

    let r = get(&app, &format!("/public-pages/{id}/edit")).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains(r#"value="Our Status""#));
    assert!(r.body.contains(&format!(
        r#"name="position_{api}" type="number" min="0" max="9999" value="2""#
    )));

    let r = get(&app, "/public-pages/999/edit").await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn delete_removes_the_page() {
    let (app, pool) = common::test_app().await;
    let api = monitor(&pool, "API").await;
    create_page(&app, &[api]).await;
    let id = page_id(&pool).await;
    assert_eq!(get(&app.logged_out(), "/status/status").await.status, 200);

    let r = post_form(&app, &format!("/public-pages/{id}/delete"), "").await;
    assert_redirect(&r, "/public-pages");
    assert_eq!(get(&app.logged_out(), "/status/status").await.status, 404);
    let r = post_form(&app, &format!("/public-pages/{id}/delete"), "").await;
    assert_eq!(r.status, 404);
}

async fn incident(pool: &SqlitePool, monitor_id: i64, started_at: i64, resolved_at: Option<i64>) {
    sqlx::query(
        "INSERT INTO incidents (monitor_id, started_at, resolved_at, error_kind)
         VALUES (?, ?, ?, 'connect')",
    )
    .bind(monitor_id)
    .bind(started_at)
    .bind(resolved_at)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn incidents_keep_open_ones_and_those_resolved_in_the_window() {
    let (app, pool) = common::test_app().await;
    let down = monitor(&pool, "Down service").await;
    let flappy = monitor(&pool, "Flappy").await;
    let old = monitor(&pool, "Old outage").await;
    create_page(&app, &[down, flappy, old]).await;
    const DAY: i64 = 86_400_000;
    let now = now_ms();
    incident(&pool, down, now - 5 * DAY, None).await;
    for i in 0..12 {
        incident(&pool, flappy, now - DAY - i * 60_000, Some(now - DAY)).await;
    }
    incident(&pool, old, now - 91 * DAY, Some(now - 89 * DAY)).await;

    let r = get(&app.logged_out(), "/status/status").await;
    assert!(
        r.body.contains("Ongoing"),
        "an open incident is never cut off"
    );
    let rows = r.body.matches("<td>Flappy</td>").count();
    assert_eq!(rows, 9, "10 incidents in all");

    sqlx::query("DELETE FROM incidents WHERE monitor_id = ?")
        .bind(flappy)
        .execute(&pool)
        .await
        .unwrap();
    let id = page_id(&pool).await;
    let r = get(&app, &format!("/public-pages/{id}/preview")).await;
    assert!(
        r.body.contains("<td>Old outage</td>"),
        "resolved within 90 days"
    );
}
