//! A free note on an incident, shown on the admin pages and, when public, on status pages.

mod common;

use common::{TestApp, get, post_form};
use sqlx::SqlitePool;
use uptime_monitoring::forms::monitor::MonitorForm;
use uptime_monitoring::models::Monitor;
use uptime_monitoring::time::now_ms;

async fn monitor_with_incident(pool: &SqlitePool) -> (i64, i64) {
    let input = MonitorForm {
        name: "API".to_string(),
        url: "https://example.com".to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap();
    let monitor = Monitor::create(pool, &input).await.unwrap();
    let started = now_ms() - 3_600_000;
    let incident = sqlx::query_scalar(
        "INSERT INTO incidents (monitor_id, started_at, resolved_at, error_kind, cause)
         VALUES (?, ?, ?, 'status', 'HTTP 502') RETURNING id",
    )
    .bind(monitor)
    .bind(started)
    .bind(started + 600_000)
    .fetch_one(pool)
    .await
    .unwrap();
    (monitor, incident)
}

async fn note_of(pool: &SqlitePool, incident: i64) -> (Option<String>, bool) {
    sqlx::query_as("SELECT note, note_public FROM incidents WHERE id = ?")
        .bind(incident)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn save(app: &TestApp, incident: i64, body: &str) -> u16 {
    post_form(app, &format!("/incidents/{incident}/note"), body)
        .await
        .status
        .as_u16()
}

#[tokio::test]
async fn note_is_saved_and_shown_on_the_admin_pages() {
    let (app, pool) = common::test_app().await;
    let (monitor, incident) = monitor_with_incident(&pool).await;

    let r = post_form(
        &app,
        &format!("/incidents/{incident}/note"),
        "note=++Disk+full+on+db-1++&public=on",
    )
    .await;
    assert_eq!(r.status, 303);
    assert_eq!(
        r.headers["location"],
        format!("/monitors/{monitor}#incident-{incident}").as_str()
    );
    assert_eq!(
        note_of(&pool, incident).await,
        (Some("Disk full on db-1".to_string()), true)
    );

    for page in [format!("/monitors/{monitor}"), "/activity".to_string()] {
        let body = get(&app, &page).await.body;
        assert!(body.contains("Disk full on db-1"), "{page}");
        assert!(body.contains("Shown on status pages"), "{page}");
        assert!(
            body.contains(&format!(r#"action="/incidents/{incident}/note""#)),
            "{page}"
        );
    }
}

#[tokio::test]
async fn blank_note_clears_it_and_its_publication() {
    let (app, pool) = common::test_app().await;
    let (_, incident) = monitor_with_incident(&pool).await;
    assert_eq!(
        save(&app, incident, "note=Investigating&public=on").await,
        303
    );
    assert_eq!(save(&app, incident, "note=+++&public=on").await, 303);
    assert_eq!(note_of(&pool, incident).await, (None, false));
}

#[tokio::test]
async fn note_is_private_unless_checked() {
    let (app, pool) = common::test_app().await;
    let (_, incident) = monitor_with_incident(&pool).await;
    assert_eq!(save(&app, incident, "note=Internal+detail").await, 303);
    assert_eq!(
        note_of(&pool, incident).await,
        (Some("Internal detail".to_string()), false)
    );
}

#[tokio::test]
async fn too_long_note_is_refused_and_unknown_incident_is_404() {
    let (app, pool) = common::test_app().await;
    let (_, incident) = monitor_with_incident(&pool).await;
    let long = format!("note={}", "a".repeat(2001));
    assert_eq!(save(&app, incident, &long).await, 422);
    assert_eq!(note_of(&pool, incident).await, (None, false));
    let max = format!("note={}", "é".repeat(2000));
    assert_eq!(save(&app, incident, &max).await, 303);

    assert_eq!(save(&app, 999, "note=x").await, 404);
    let r = post_form(
        &app.logged_out(),
        &format!("/incidents/{incident}/note"),
        "note=x",
    )
    .await;
    assert_eq!(r.status, 303);
    assert_eq!(r.headers["location"], "/login");
}

#[tokio::test]
async fn status_page_shows_only_public_notes() {
    let (app, pool) = common::test_app().await;
    let (monitor, incident) = monitor_with_incident(&pool).await;
    let body =
        format!("title=Status&slug=status&published=on&monitor_{monitor}=on&position_{monitor}=1");
    assert_eq!(
        post_form(&app, "/public-pages/new", &body).await.status,
        303
    );

    assert_eq!(save(&app, incident, "note=Secret+host+db-1").await, 303);
    let page = get(&app.logged_out(), "/status/status").await.body;
    assert!(!page.contains("Secret host"));

    // The page is cached for a minute: a fresh app renders it again.
    let (app, pool) = common::test_app().await;
    let (monitor, incident) = monitor_with_incident(&pool).await;
    let body =
        format!("title=Status&slug=status&published=on&monitor_{monitor}=on&position_{monitor}=1");
    assert_eq!(
        post_form(&app, "/public-pages/new", &body).await.status,
        303
    );
    assert_eq!(
        save(&app, incident, "note=Provider+outage&public=on").await,
        303
    );
    let page = get(&app.logged_out(), "/status/status").await.body;
    assert!(page.contains("Provider outage"), "{page}");
}
