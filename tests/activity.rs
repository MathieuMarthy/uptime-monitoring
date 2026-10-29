mod common;

use common::{TestApp, get, post_form};
use sqlx::SqlitePool;
use uptime_monitoring::time::now_ms;

const MINUTE: i64 = 60_000;

async fn create(app: &TestApp, name: &str) {
    let body = format!(
        "name={name}&url=https%3A%2F%2Fexample.com&method=GET&accepted_statuses=200-299&interval_s=60\
         &retry_interval_s=20&timeout_s=10&failure_threshold=2&tls_expiry_warn_days=14&headers=&body=\
         &keyword=&follow_redirects=on&active=on"
    );
    assert_eq!(post_form(app, "/monitors/new", &body).await.status, 303);
}

async fn exec(pool: &SqlitePool, sql: &'static str, now: i64) {
    sqlx::query(sql).bind(now).execute(pool).await.unwrap();
}

/// API (1) had a 10-minute outage an hour ago, alerted on "Ops hook"; Blog (2) failed one check
/// 30 minutes ago and got a certificate warning 2 hours ago.
async fn app_with_history() -> (TestApp, SqlitePool, i64) {
    let (app, pool) = common::test_app().await;
    create(&app, "API").await;
    create(&app, "Blog").await;
    let now = now_ms();
    exec(
        &pool,
        "INSERT INTO checks (monitor_id, checked_at, ok, latency_ms, status_code, error_kind, error,
                             covered_ms, down)
         VALUES (1, ?1 - 3600000, 0, 30, 503, 'status', 'HTTP 503 Service Unavailable', 60000, 0),
                (1, ?1 - 3540000, 0, 31, 503, 'status', 'HTTP 503 Service Unavailable', 60000, 1),
                (1, ?1 - 3000000, 1, 40, 200, NULL, NULL, 60000, 1),
                (2, ?1 - 1800000, 0, NULL, NULL, 'dns', 'DNS lookup failed', 60000, 0)",
        now,
    )
    .await;
    exec(
        &pool,
        "INSERT INTO incidents (monitor_id, started_at, resolved_at, error_kind, cause)
         VALUES (1, ?1 - 3600000, ?1 - 3000000, 'status', 'HTTP 503 Service Unavailable')",
        now,
    )
    .await;
    exec(
        &pool,
        "INSERT INTO notification_channels (name, kind, config, enabled, created_at)
         VALUES ('Ops hook', 'webhook', '{\"url\":\"https://hooks.example.com\"}', 1, ?1)",
        now,
    )
    .await;
    exec(
        &pool,
        "INSERT INTO notification_log (channel_id, monitor_id, event, sent_at, ok, error)
         VALUES (1, 1, 'down', ?1 - 3540000, 1, NULL),
                (1, 1, 'up', ?1 - 2990000, 0, 'HTTP 500 Internal Server Error'),
                (1, 2, 'tls_expiring', ?1 - 7200000, 1, NULL)",
        now,
    )
    .await;
    exec(
        &pool,
        "UPDATE monitor_state SET tls_warned_at = ?1 - 7200000, tls_warned_for = ?1 + 9 * 86400000 + 3600000,
                                  tls_expires_at = ?1 + 9 * 86400000 + 3600000
         WHERE monitor_id = 2",
        now,
    )
    .await;
    (app, pool, now)
}

fn position(body: &str, needle: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("missing {needle:?} in {body}"))
}

#[tokio::test]
async fn activity_needs_a_session() {
    let (app, _, _) = app_with_history().await;
    let r = get(&app.logged_out(), "/activity").await;
    assert_eq!(r.status, 303);
    assert_eq!(r.headers["location"], "/login");
}

#[tokio::test]
async fn activity_lists_events_latest_first_with_their_details() {
    let (app, _, _) = app_with_history().await;
    let r = get(&app, "/activity").await;
    assert_eq!(r.status, 200);
    let body = r.body;
    let failure = position(&body, "Blog failed a check");
    let outage = position(&body, "API was down 10 min");
    let certificate = position(&body, "Blog certificate expires soon");
    assert!(failure < outage && outage < certificate, "latest first");

    // The outage unfolds to its failed checks and alerts.
    assert_eq!(body.matches("HTTP 503 Service Unavailable").count(), 3);
    assert!(body.contains("Ops hook"));
    assert!(body.contains("failed: HTTP 500 Internal Server Error"));
    assert!(body.contains("DNS lookup failed"));
    assert!(body.contains("9 days left"));
    // The first failure of the outage belongs to it, not to the failed checks.
    assert!(!body.contains("API failed a check"));
}

#[tokio::test]
async fn activity_filters_by_type_and_monitor() {
    let (app, _, _) = app_with_history().await;
    let cases: [(&str, &[&str], &[&str]); 4] = [
        (
            "type=outages",
            &["API was down"],
            &["Blog failed", "certificate expires"],
        ),
        (
            "type=checks",
            &["Blog failed a check"],
            &["API was down", "certificate expires"],
        ),
        (
            "type=certificates",
            &["Blog certificate expires soon"],
            &["API was down", "Blog failed"],
        ),
        (
            "monitor=2",
            &["Blog failed a check", "Blog certificate"],
            &["API was down"],
        ),
    ];
    for (query, shown, hidden) in cases {
        let body = get(&app, &format!("/activity?{query}")).await.body;
        for needle in shown {
            assert!(body.contains(needle), "{query}: missing {needle}");
        }
        for needle in hidden {
            assert!(!body.contains(needle), "{query}: shows {needle}");
        }
    }
}

#[tokio::test]
async fn activity_ignores_bad_filters_and_limits_the_period() {
    let (app, pool, now) = app_with_history().await;
    let r = get(&app, "/activity?type=bogus&monitor=abc&days=-3&before=x").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("API was down"));

    exec(
        &pool,
        "UPDATE incidents SET started_at = started_at - 10 * 86400000,
                              resolved_at = resolved_at - 10 * 86400000",
        now,
    )
    .await;
    assert!(
        !get(&app, "/activity").await.body.contains("API was down"),
        "7 days by default"
    );
    assert!(
        get(&app, "/activity?days=30")
            .await
            .body
            .contains("API was down")
    );
}

#[tokio::test]
async fn activity_pages_through_older_events() {
    let (app, pool) = common::test_app().await;
    create(&app, "API").await;
    let now = now_ms();
    for i in 1..=60 {
        sqlx::query(
            "INSERT INTO checks (monitor_id, checked_at, ok, error_kind, error, covered_ms, down)
             VALUES (1, ?, 0, 'timeout', ?, 60000, 0)",
        )
        .bind(now - i * MINUTE)
        .bind(format!("Failure number {i}."))
        .execute(&pool)
        .await
        .unwrap();
    }
    let body = get(&app, "/activity").await.body;
    assert_eq!(body.matches("API failed a check").count(), 50);
    assert!(body.contains("Failure number 50."));
    assert!(!body.contains("Failure number 51."));
    let next = format!("/activity?before={}-1-1", now - 50 * MINUTE);
    assert!(
        body.contains(&next.replace('&', "&amp;")),
        "older link: {body}"
    );

    let body = get(&app, &next).await.body;
    assert_eq!(body.matches("API failed a check").count(), 10);
    assert!(body.contains("Failure number 51."));
    assert!(!body.contains("/activity?before="), "no older page");
}

#[tokio::test]
async fn alerts_stay_with_their_incident_when_a_monitor_flaps() {
    let (app, pool) = common::test_app().await;
    create(&app, "API").await;
    let now = now_ms();
    // A: down from -20 min to -10 min. B: down 30 s after A resolved, still open. Each alert lands
    // a minute after its transition, so A's "up" is sent after B started.
    exec(
        &pool,
        "INSERT INTO incidents (monitor_id, started_at, resolved_at, error_kind, cause)
         VALUES (1, ?1 - 1200000, ?1 - 600000, 'status', 'First outage'),
                (1, ?1 - 570000, NULL, 'status', 'Second outage')",
        now,
    )
    .await;
    exec(
        &pool,
        "INSERT INTO notification_channels (name, kind, config, enabled, created_at)
         VALUES ('Ops hook', 'webhook', '{\"url\":\"https://hooks.example.com\"}', 1, ?1)",
        now,
    )
    .await;
    exec(
        &pool,
        "INSERT INTO notification_log (channel_id, monitor_id, event, sent_at, ok, error)
         VALUES (1, 1, 'down', ?1 - 1140000, 1, NULL),
                (1, 1, 'up', ?1 - 540000, 1, NULL),
                (1, 1, 'down', ?1 - 510000, 1, NULL)",
        now,
    )
    .await;
    for uri in ["/activity", "/monitors/1"] {
        let body = get(&app, uri).await.body;
        let second = position(&body, "Second outage");
        let first = position(&body, "First outage");
        assert!(second < first, "{uri}: latest first");
        let (second, first) = body.split_at(first);
        assert_eq!(
            second.matches("down alert").count(),
            1,
            "{uri}: B has its down alert only"
        );
        assert_eq!(
            second.matches("back up alert").count(),
            0,
            "{uri}: A's up is not B's"
        );
        assert_eq!(
            first.matches("down alert").count(),
            1,
            "{uri}: A has its down alert only"
        );
        assert_eq!(
            first.matches("back up alert").count(),
            1,
            "{uri}: A has its up alert"
        );
    }
}

#[tokio::test]
async fn events_sharing_a_time_are_not_lost_between_pages() {
    let (app, pool) = common::test_app().await;
    create(&app, "API").await;
    create(&app, "Blog").await;
    let now = now_ms();
    // 49 failures of API one minute apart, then API and Blog failing at the same time: the 50th
    // event ends the first page, its twin must open the second.
    for i in 1..=50 {
        sqlx::query(
            "INSERT INTO checks (monitor_id, checked_at, ok, error_kind, error, covered_ms, down)
             VALUES (1, ?, 0, 'timeout', ?, 60000, 0)",
        )
        .bind(now - i * MINUTE)
        .bind(format!("API failure {i}."))
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO checks (monitor_id, checked_at, ok, error_kind, error, covered_ms, down)
         VALUES (2, ?, 0, 'dns', 'Blog failure.', 60000, 0)",
    )
    .bind(now - 50 * MINUTE)
    .execute(&pool)
    .await
    .unwrap();

    let first = get(&app, "/activity").await.body;
    let next = first
        .split("href=\"")
        .find_map(|s| s.strip_prefix("/activity?before="))
        .map(|s| format!("/activity?before={}", &s[..s.find('"').unwrap()]))
        .expect("older link");
    let second = get(&app, &next).await.body;
    let shown = |needle: &str| first.contains(needle) as u8 + second.contains(needle) as u8;
    assert_eq!(shown("API failure 50."), 1);
    assert_eq!(shown("Blog failure."), 1);
}
