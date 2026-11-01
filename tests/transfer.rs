mod common;

use common::{get, post_form};
use serde_json::{Value, json};
use sqlx::SqlitePool;

/// A channel, two monitors (one push, linked to it), a public page and a weekly window.
async fn seed(pool: &SqlitePool) {
    for sql in [
        "INSERT INTO notification_channels (name, kind, config, enabled, group_alerts, created_at)
         VALUES ('Ops', 'ntfy', '{\"server\":\"https://ntfy.sh\",\"topic\":\"ops\",\"token\":\"tk_secret\"}', 1, 0, 0)",
        "INSERT INTO monitors (name, name_key, group_name, url, method, headers, accepted_statuses,
                               interval_s, retry_interval_s, timeout_s, failure_threshold, latency_warn_ms,
                               created_at, updated_at)
         VALUES ('API', 'api', 'Web', 'https://api.example.com/health', 'POST', '{\"X-Key\":\"1\"}',
                 '200-299,301', 120, 30, 15, 2, 800, 0, 0)",
        "INSERT INTO monitors (name, name_key, kind, url, push_token, grace_s, paused, created_at, updated_at)
         VALUES ('Backup job', 'backup job', 'push', '',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 300, 1, 0, 0)",
        "INSERT INTO monitor_state (monitor_id) VALUES (1), (2)",
        "INSERT INTO monitor_channels (monitor_id, channel_id) VALUES (2, 1)",
        "INSERT INTO status_pages (slug, title, description, published, created_at, updated_at)
         VALUES ('main', 'Main', 'All services', 1, 0, 0)",
        "INSERT INTO status_page_monitors (page_id, monitor_id, display_name, position)
         VALUES (1, 2, 'Backups', 0), (1, 1, NULL, 1)",
        "INSERT INTO maintenances (title, all_monitors, show_on_status_pages, weekdays, local_start,
                                   duration_min, created_at, updated_at)
         VALUES ('Nightly', 0, 1, 3, 150, 30, 0, 0)",
        "INSERT INTO maintenance_monitors (maintenance_id, monitor_id) VALUES (1, 1)",
    ] {
        sqlx::query(sql).execute(pool).await.unwrap();
    }
}

async fn export(app: &common::TestApp) -> Value {
    let r = get(app, "/settings/export").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.headers["content-type"], "application/json");
    let disposition = r.headers["content-disposition"].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"uptime-monitoring-"),
        "{disposition}"
    );
    serde_json::from_str(&r.body).unwrap()
}

#[tokio::test]
async fn export_holds_the_configuration_linked_by_name() {
    let (app, pool) = common::test_app().await;
    seed(&pool).await;
    let doc = export(&app).await;

    assert_eq!(doc["format"], "uptime-monitoring");
    assert_eq!(doc["version"], 1);
    assert_eq!(
        doc["channels"],
        json!([{
            "name": "Ops", "kind": "ntfy", "enabled": true, "group_alerts": false,
            "config": {"server": "https://ntfy.sh", "topic": "ops", "token": "tk_secret"}
        }])
    );

    let monitors = doc["monitors"].as_array().unwrap();
    assert_eq!(monitors.len(), 2);
    let api = &monitors[0];
    assert_eq!(api["name"], "API");
    assert_eq!(api["group"], "Web");
    assert_eq!(api["method"], "POST");
    assert_eq!(api["headers"], "{\"X-Key\":\"1\"}");
    assert_eq!(api["accepted_statuses"], "200-299,301");
    assert_eq!(api["interval_s"], 120);
    assert_eq!(api["latency_warn_ms"], 800);
    assert_eq!(api["channels"], json!([]));
    let push = &monitors[1];
    assert_eq!(push["kind"], "push");
    assert_eq!(push["paused"], true);
    assert_eq!(push["push_token"], "a".repeat(64));
    assert_eq!(push["channels"], json!(["Ops"]));

    assert_eq!(
        doc["status_pages"],
        json!([{
            "slug": "main", "title": "Main", "description": "All services", "published": true,
            "monitors": [
                {"monitor": "Backup job", "display_name": "Backups"},
                {"monitor": "API", "display_name": null}
            ]
        }])
    );
    assert_eq!(
        doc["maintenances"],
        json!([{
            "title": "Nightly", "all_monitors": false, "show_on_status_pages": true,
            "schedule": {"kind": "weekly", "weekdays": 3, "local_start": 150, "duration_min": 30},
            "monitors": ["API"]
        }])
    );
}

#[tokio::test]
async fn export_requires_a_session() {
    let (app, _) = common::test_app().await;
    let r = get(&app.logged_out(), "/settings/export").await;
    assert_eq!(r.status, 303);
}

#[tokio::test]
async fn settings_page_links_to_export_and_backup() {
    let (app, _) = common::test_app().await;
    let r = get(&app, "/settings").await;
    assert!(r.body.contains(r#"href="/settings/export""#));
    assert!(r.body.contains(r#"href="/settings/backup""#));
    let _ = post_form;
}

#[tokio::test]
async fn backup_requires_a_session() {
    let (app, _) = common::test_app().await;
    let r = get(&app.logged_out(), "/settings/backup").await;
    assert_eq!(r.status, 303);
}

/// A file database: an in-memory one copies into memory, not to disk.
#[tokio::test]
async fn backup_streams_a_copy_and_leaves_no_file_behind() {
    let dir = std::env::temp_dir().join(format!("um-backup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let url = format!("sqlite://{}", dir.join("uptime.db").display());
    let (app, pool) = common::test_app_on(&url).await;
    seed(&pool).await;

    let r = get(&app, "/settings/backup").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.headers["content-type"], "application/vnd.sqlite3");
    let disposition = r.headers["content-disposition"].to_str().unwrap();
    assert!(disposition.ends_with(".db\""), "{disposition}");
    assert!(r.body.starts_with("SQLite format 3\0"));
    assert!(r.body.contains("api.example.com"));
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with(".backup-"))
        .collect();
    assert_eq!(left, Vec::<String>::new());
    pool.close().await;
    std::fs::remove_dir_all(&dir).unwrap();
}
