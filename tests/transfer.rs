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
        "INSERT INTO monitors (name, name_key, kind, url, push_token, grace_s, paused, retry_interval_s,
                               tls_expiry_warn_days, created_at, updated_at)
         VALUES ('Backup job', 'backup job', 'push', '',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 300, 1, 60, NULL, 0, 0)",
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
    let r = get(&app, "/settings?section=backup").await;
    assert!(r.body.contains(r#"href="/settings/export""#));
    assert!(r.body.contains(r#"href="/settings/backup""#));
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

async fn import(app: &common::TestApp, document: &str) -> common::TestResponse {
    let body = format!(
        "document={}",
        form_urlencoded::byte_serialize(document.as_bytes()).collect::<String>()
    );
    post_form(app, "/settings/import", &body).await
}

/// The export without what differs between two instances.
fn comparable(mut doc: Value) -> Value {
    doc["exported_at"] = Value::Null;
    doc
}

#[tokio::test]
async fn import_of_an_export_rebuilds_the_configuration() {
    let (source, pool) = common::test_app().await;
    seed(&pool).await;
    let exported = export(&source).await;

    let (target, _) = common::test_app().await;
    let r = import(&target, &exported.to_string()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("5 added"), "{}", r.body);
    assert_eq!(comparable(export(&target).await), comparable(exported));
}

#[tokio::test]
async fn import_only_adds_and_skips_what_exists() {
    let (app, pool) = common::test_app().await;
    seed(&pool).await;
    let before = export(&app).await;
    let mut doc = before.clone();
    doc["monitors"][0]["url"] = json!("https://changed.example.com");
    doc["monitors"][1]["name"] = json!("NEW job");
    doc["monitors"][1]["push_token"] = json!("a".repeat(64));

    let r = import(&app, &doc.to_string()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("1 added"), "{}", r.body);
    assert!(r.body.contains("API"), "{}", r.body);
    assert!(r.body.contains("already exists"), "{}", r.body);

    let after = export(&app).await;
    let monitors = after["monitors"].as_array().unwrap();
    assert_eq!(monitors.len(), 3);
    assert_eq!(monitors[0]["url"], "https://api.example.com/health");
    let new_job = &monitors[2];
    assert_eq!(new_job["name"], "NEW job");
    assert_eq!(new_job["channels"], json!(["Ops"]));
    // The token belongs to the existing monitor: the new one gets its own.
    assert_ne!(new_job["push_token"], json!("a".repeat(64)));
    assert_eq!(after["channels"], before["channels"]);
    assert_eq!(after["status_pages"], before["status_pages"]);
}

#[tokio::test]
async fn import_skips_invalid_entries_with_their_reason() {
    let (app, pool) = common::test_app().await;
    let doc = json!({
        "format": "uptime-monitoring", "version": 1,
        "monitors": [
            {"name": "Good", "url": "https://good.example.com"},
            {"name": "Bad", "url": "not a url"},
            {"name": "Slow", "url": "https://slow.example.com", "interval_s": 1}
        ]
    });
    let r = import(&app, &doc.to_string()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("1 added"), "{}", r.body);
    assert!(r.body.contains("Bad"), "{}", r.body);
    assert!(r.body.contains("Slow"), "{}", r.body);
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM monitors")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(names, ["Good"]);
}

#[tokio::test]
async fn import_rejects_an_unknown_document() {
    let (app, pool) = common::test_app().await;
    for document in [
        "not json",
        "{\"hello\": 1}",
        "{\"format\":\"uptime-monitoring\",\"version\":99}",
    ] {
        let r = import(&app, document).await;
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(r.body.contains("field-error"), "{document}: {}", r.body);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM monitors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn import_accepts_documents_larger_than_a_form() {
    let (app, pool) = common::test_app().await;
    let monitors: Vec<Value> = (0..400)
        .map(|i| {
            json!({"name": format!("Site {i}"), "url": format!("https://site{i}.example.com/"),
                   "body": "x".repeat(500), "method": "POST"})
        })
        .collect();
    let doc = json!({"format": "uptime-monitoring", "version": 1, "monitors": monitors});
    let r = import(&app, &doc.to_string()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM monitors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 400);
}

const KUMA_BACKUP: &str = r#"{
  "version": "1.23.11",
  "notificationList": [
    {"id": 1, "name": "Discord ops", "active": 1, "isDefault": 0,
     "config": "{\"name\":\"Discord ops\",\"type\":\"discord\",\"discordWebhookUrl\":\"https://discord.com/api/webhooks/1/abc\"}"},
    {"id": 2, "name": "Phone", "active": 1, "isDefault": 0,
     "config": "{\"type\":\"ntfy\",\"ntfyserverurl\":\"https://ntfy.example.com\",\"ntfytopic\":\"alerts\",\"ntfyAuthenticationMethod\":\"accessToken\",\"ntfyaccesstoken\":\"tk_abc\"}"},
    {"id": 3, "name": "Bot", "active": 0, "isDefault": 0,
     "config": "{\"type\":\"telegram\",\"telegramBotToken\":\"123:ABC\",\"telegramChatID\":\"-100\"}"},
    {"id": 4, "name": "Mail", "active": 1, "isDefault": 0,
     "config": "{\"type\":\"smtp\",\"smtpHost\":\"mail.example.com\",\"smtpPort\":465,\"smtpSecure\":true,\"smtpUsername\":\"up\",\"smtpPassword\":\"pw\",\"smtpFrom\":\"Kuma <up@example.com>\",\"smtpTo\":\"ops@example.com\",\"smtpCC\":\"dev@example.com\",\"smtpBCC\":\"\"}"},
    {"id": 5, "name": "Pager", "active": 1, "isDefault": 0,
     "config": "{\"type\":\"pushover\"}"}
  ],
  "monitorList": [
    {"id": 10, "name": "Production", "type": "group", "parent": null, "active": 1,
     "notificationIDList": {}},
    {"id": 11, "name": "Website", "type": "http", "url": "https://www.example.com", "method": "GET",
     "interval": 60, "retryInterval": 30, "maxretries": 2, "timeout": 48, "active": 1,
     "ignoreTls": 0, "maxredirects": 10, "accepted_statuscodes": ["200-299", "301"],
     "expiryNotification": 1, "parent": 10, "headers": "{\"X-Test\": \"1\"}", "body": null,
     "notificationIDList": {"1": true, "4": true}},
    {"id": 12, "name": "Search", "type": "keyword", "url": "https://search.example.com",
     "method": "POST", "body": "{}", "keyword": "results", "invertKeyword": false,
     "interval": 300, "retryInterval": 600, "maxretries": 0, "timeout": 24, "active": 0,
     "ignoreTls": 1, "maxredirects": 0, "accepted_statuscodes": ["200-299"],
     "expiryNotification": 0, "parent": null, "notificationIDList": {"2": true, "3": true}},
    {"id": 13, "name": "Cron", "type": "push", "pushToken": "kuma123", "interval": 3600,
     "retryInterval": 3600, "maxretries": 0, "active": 1, "parent": null,
     "notificationIDList": {}},
    {"id": 14, "name": "Router", "type": "ping", "hostname": "10.0.0.1", "interval": 60,
     "active": 1, "parent": null, "notificationIDList": {}},
    {"id": 15, "name": "Not here", "type": "keyword", "url": "https://x.example.com",
     "keyword": "error", "invertKeyword": true, "interval": 60, "active": 1, "parent": null,
     "notificationIDList": {}}
  ]
}"#;

#[tokio::test]
async fn import_of_an_uptime_kuma_backup() {
    let (app, _) = common::test_app().await;
    let r = import(&app, KUMA_BACKUP).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("7 added"), "{}", r.body);
    for skipped in ["Pager", "Router", "Not here"] {
        assert!(r.body.contains(skipped), "{skipped}: {}", r.body);
    }

    let doc = export(&app).await;
    assert_eq!(
        doc["channels"],
        json!([
            {"name": "Bot", "kind": "telegram", "enabled": false, "group_alerts": true,
             "config": {"token": "123:ABC", "chat_id": "-100"}},
            {"name": "Discord ops", "kind": "discord", "enabled": true, "group_alerts": true,
             "config": {"url": "https://discord.com/api/webhooks/1/abc"}},
            {"name": "Mail", "kind": "email", "enabled": true, "group_alerts": true,
             "config": {"host": "mail.example.com", "port": 465, "security": "tls",
                        "username": "up", "password": "pw", "from": "Kuma <up@example.com>",
                        "to": ["ops@example.com", "dev@example.com"]}},
            {"name": "Phone", "kind": "ntfy", "enabled": true, "group_alerts": true,
             "config": {"server": "https://ntfy.example.com", "topic": "alerts", "token": "tk_abc"}}
        ])
    );
    let monitors = doc["monitors"].as_array().unwrap();
    let names: Vec<&str> = monitors
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Cron", "Search", "Website"]);

    let cron = &monitors[0];
    assert_eq!(cron["kind"], "push");
    assert_eq!(cron["interval_s"], 3600);
    assert_eq!(cron["failure_threshold"], 1);
    assert_eq!(cron["push_token"].as_str().unwrap().len(), 64);

    let search = &monitors[1];
    assert_eq!(search["method"], "POST");
    assert_eq!(search["keyword"], "results");
    assert_eq!(search["body"], "{}");
    assert_eq!(search["retry_interval_s"], 300);
    assert_eq!(search["timeout_s"], 24);
    assert_eq!(search["failure_threshold"], 1);
    assert_eq!(search["ignore_tls_errors"], true);
    assert_eq!(search["follow_redirects"], false);
    assert_eq!(search["tls_expiry_warn_days"], Value::Null);
    assert_eq!(search["paused"], true);
    assert_eq!(search["channels"], json!(["Bot", "Phone"]));

    let website = &monitors[2];
    assert_eq!(website["group"], "Production");
    assert_eq!(website["url"], "https://www.example.com");
    assert_eq!(website["accepted_statuses"], "200-299,301");
    assert_eq!(website["headers"], "{\"X-Test\": \"1\"}");
    assert_eq!(website["interval_s"], 60);
    assert_eq!(website["retry_interval_s"], 30);
    assert_eq!(website["timeout_s"], 48);
    assert_eq!(website["failure_threshold"], 3);
    assert_eq!(website["tls_expiry_warn_days"], 14);
    assert_eq!(website["paused"], false);
    assert_eq!(website["channels"], json!(["Discord ops", "Mail"]));
}

#[tokio::test]
async fn settings_page_has_the_import_form() {
    let (app, _) = common::test_app().await;
    let r = get(&app, "/settings?section=backup").await;
    assert!(r.body.contains(r#"action="/settings/import""#));
    assert!(r.body.contains(r#"name="document""#));
}

#[tokio::test]
async fn import_keeps_windows_sharing_a_title_and_skips_exact_copies() {
    let (app, pool) = common::test_app().await;
    seed(&pool).await;
    let window = |start: i64| {
        json!({"title": "Nightly", "monitors": ["API"],
               "schedule": {"kind": "weekly", "weekdays": 3, "local_start": start, "duration_min": 30}})
    };
    let doc = json!({"format": "uptime-monitoring", "version": 1,
                     "maintenances": [window(150), window(300), window(300)]});
    let r = import(&app, &doc.to_string()).await;
    assert!(r.body.contains("1 added, 2 skipped"), "{}", r.body);
    let starts: Vec<i64> =
        sqlx::query_scalar("SELECT local_start FROM maintenances ORDER BY local_start")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(starts, [150, 300]);
}

#[tokio::test]
async fn import_keeps_one_off_windows_across_a_clock_change() {
    let (app, pool) = common::test_app().await;
    seed(&pool).await;
    sqlx::query("UPDATE settings SET timezone = 'Europe/Paris'")
        .execute(&pool)
        .await
        .unwrap();
    // 2026-10-25 00:50 to 01:10 UTC: 02:50 CEST to 02:10 CET.
    let (starts_at, ends_at) = (1_792_889_400_000_i64, 1_792_890_600_000_i64);
    let doc = json!({"format": "uptime-monitoring", "version": 1, "maintenances": [
        {"title": "Fall back", "monitors": ["API"],
         "schedule": {"kind": "once", "starts_at": starts_at, "ends_at": ends_at}}]});
    let r = import(&app, &doc.to_string()).await;
    assert!(r.body.contains("1 added"), "{}", r.body);
    let stored: (i64, i64) =
        sqlx::query_as("SELECT starts_at, ends_at FROM maintenances WHERE title = 'Fall back'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, (starts_at, ends_at));
}

#[tokio::test]
async fn import_tells_which_channels_a_monitor_lost() {
    let (app, _) = common::test_app().await;
    let doc = json!({"format": "uptime-monitoring", "version": 1, "monitors": [
        {"name": "Site", "url": "https://site.example.com", "channels": ["Gone"]}]});
    let r = import(&app, &doc.to_string()).await;
    assert!(r.body.contains("1 added"), "{}", r.body);
    assert!(r.body.contains("Gone"), "{}", r.body);
}

#[tokio::test]
async fn import_says_when_a_document_is_malformed_rather_than_newer() {
    let (app, _) = common::test_app().await;
    let r = import(&app, r#"{"format":"uptime-monitoring","version":"1"}"#).await;
    assert!(!r.body.contains("newer version"), "{}", r.body);
    let r = import(&app, r#"{"format":"uptime-monitoring","version":2}"#).await;
    assert!(r.body.contains("newer version"), "{}", r.body);
}

#[tokio::test]
async fn kuma_notifications_sharing_a_name_stay_apart() {
    let (app, _) = common::test_app().await;
    let backup = json!({
        "notificationList": [
            {"id": 1, "name": "Ops", "config": "{\"type\":\"webhook\",\"webhookURL\":\"https://a.example.com\"}"},
            {"id": 2, "name": "Ops", "config": "{\"type\":\"webhook\",\"webhookURL\":\"https://b.example.com\"}"},
            {"id": 3, "name": "Topicless", "config": "{\"type\":\"ntfy\",\"ntfyserverurl\":\"https://ntfy.sh\"}"}
        ],
        "monitorList": [
            {"id": 5, "name": "Site", "type": "http", "url": "https://site.example.com",
             "notificationIDList": {"2": true}}
        ]
    });
    let r = import(&app, &backup.to_string()).await;
    assert!(r.body.contains("Topic is required"), "{}", r.body);
    let doc = export(&app).await;
    let channels: Vec<(&str, &str)> = doc["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap(),
                c["config"]["url"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        channels,
        [
            ("Ops", "https://a.example.com"),
            ("Ops (2)", "https://b.example.com")
        ]
    );
    assert_eq!(doc["monitors"][0]["channels"], json!(["Ops (2)"]));
}
