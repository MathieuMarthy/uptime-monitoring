mod common;

use common::{TestApp, TestResponse, get, post_form};
use sqlx::SqlitePool;

const DEFAULTS: &str = "method=GET&accepted_statuses=200-299&interval_s=60&retry_interval_s=20&timeout_s=10\
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
async fn dashboard_headline_names_down_monitors_and_lists_problems_first() {
    let (app, pool) = common::test_app().await;
    for name in ["a", "b", "c", "d"] {
        create(&app, name).await;
    }
    post_form(&app, "/monitors/1/toggle-pause", "").await;
    sqlx::query(
        "UPDATE monitor_state SET status = CASE monitor_id WHEN 2 THEN 'down' WHEN 4 THEN 'up'
                                                          ELSE status END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let r = get(&app, "/").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("1 monitor is down"), "{}", r.body);
    assert!(
        r.body.contains(r#"<a href="/monitors/2">b</a>."#),
        "{}",
        r.body
    );
    assert!(r.body.contains("1 pending, 1 up, 1 paused."), "{}", r.body);
    // The state is written out, not only shown by the lamp's colour.
    for state in ["Down", "Pending", "Paused"] {
        let needle = format!(r#"state-{}">{state}<"#, state.to_lowercase());
        assert!(r.body.contains(&needle), "missing {needle}");
    }
    let position = |name: &str| {
        r.body
            .find(&format!(r#"data-search="{name} "#))
            .unwrap_or_else(|| panic!("no row for {name}"))
    };
    assert!(position("b") < position("c"), "down before pending");
    assert!(position("c") < position("d"), "pending before up");
    assert!(position("d") < position("a"), "paused last");
}

#[tokio::test]
async fn dashboard_switches_to_90_days() {
    let (app, _) = common::test_app().await;
    create(&app, "API").await;
    let body = get(&app, "/?range=90d").await.body;
    assert_eq!(body.matches(r#"class="tick tick-"#).count(), 90);
    assert!(body.contains("today"));
    let body = get(&app, "/?range=bogus").await.body;
    assert_eq!(body.matches(r#"class="tick tick-"#).count(), 24);
    assert!(body.contains("now"));
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

#[tokio::test]
async fn dashboard_shows_uptime_and_hourly_heartbeat() {
    let (app, pool) = common::test_app().await;
    create(&app, "API").await;
    create(&app, "Idle").await;
    let now = uptime_monitoring::time::now_ms();
    for (i, down) in [false, false, true, false].into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO checks (monitor_id, checked_at, ok, latency_ms, covered_ms, down)
             VALUES (1, ?, ?, 120, 60000, ?)",
        )
        .bind(now - 4 + i as i64)
        .bind(!down)
        .bind(down)
        .execute(&pool)
        .await
        .unwrap();
    }

    let body = get(&app, "/").await.body;
    let (api, idle) = body.split_once("Idle").unwrap();
    assert!(api.contains("75.00%"), "{api}");
    assert_eq!(api.matches("tick tick-down").count(), 1);
    assert_eq!(api.matches("tick tick-none").count(), 23);
    assert_eq!(idle.matches("tick tick-none").count(), 24);
    assert!(!idle.contains('%'));
    assert!(api.contains(r#"data-search="API https://example.com""#));
    assert!(
        body.contains(r#"id="theme-switch""#),
        "theme switch in the header"
    );
    for script in [
        "/static/js/live.js",
        "/static/js/search.js",
        "/static/js/theme.js",
    ] {
        assert!(body.contains(script));
        assert_eq!(get(&app, script).await.status, 200, "{script}");
    }
}

#[tokio::test]
async fn detail_shows_history_incidents_and_recent_checks() {
    use uptime_monitoring::time::{day_key, day_of, now_ms};
    let (app, pool) = common::test_app().await;
    create(&app, "API").await;
    let now = now_ms();
    let two_days_ago = day_of(&jiff::tz::TimeZone::UTC, now)
        .checked_sub(jiff::Span::new().days(2))
        .unwrap();
    // A rolled-up day at 99 % uptime, then two checks today, the second one down.
    sqlx::query(
        "INSERT INTO daily_stats (monitor_id, day, monitored_ms, downtime_ms, checks_total,
                                  checks_failed, latency_avg_ms, latency_p95_ms)
         VALUES (1, ?, 86400000, 864000, 1440, 10, 100, 200)",
    )
    .bind(day_key(two_days_ago))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO checks (monitor_id, checked_at, ok, latency_ms, status_code, error_kind, error,
                             covered_ms, down)
         VALUES (1, ?1 - 2, 1, 150, 200, NULL, NULL, 60000, 0),
                (1, ?1 - 1, 0, 90, 503, 'status', 'HTTP 503 Service Unavailable', 60000, 1)",
    )
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO incidents (monitor_id, started_at, error_kind, cause)
         VALUES (1, ?, 'status', 'HTTP 503 Service Unavailable')",
    )
    .bind(now - 1)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE monitor_state SET status = 'down' WHERE monitor_id = 1")
        .execute(&pool)
        .await
        .unwrap();

    let body = get(&app, "/monitors/1").await.body;
    assert!(body.contains("50.00%"), "24 h uptime");
    assert!(body.contains("98.93%"), "30-day uptime");
    assert!(body.contains("120 ms"), "24 h average latency");
    assert_eq!(body.matches(r#"class="tick tick-"#).count(), 24 + 90);
    assert!(body.contains("Ongoing"));
    assert!(body.contains("Down for "), "status line");
    // The status line, the incident, its failed check and the latest checks.
    assert_eq!(body.matches("HTTP 503 Service Unavailable").count(), 4);
    assert!(
        body.contains(r#"<span class="lat latency"#),
        "latency chart: {body}"
    );
    let hourly = body
        .split(r#"id="hourly-figures""#)
        .nth(1)
        .expect("hourly figures");
    let hourly = &hourly[..hourly.find("</table>").unwrap()];
    assert_eq!(
        hourly.matches("<tr>").count(),
        1 + 24,
        "a row per hour, for keyboard users"
    );
    for id in [
        "live-status",
        "live-history",
        "live-incidents",
        "live-checks",
    ] {
        assert!(body.contains(&format!(r#"id="{id}" data-live"#)), "{id}");
    }
}

async fn add_channel(pool: &SqlitePool, name: &str, enabled: bool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO notification_channels (name, kind, config, enabled, created_at)
         VALUES (?, 'webhook', '{\"url\":\"https://hooks.example.com\"}', ?, 0) RETURNING id",
    )
    .bind(name)
    .bind(enabled)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn linked_channels(pool: &SqlitePool, monitor_id: i64) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT channel_id FROM monitor_channels WHERE monitor_id = ? ORDER BY channel_id",
    )
    .bind(monitor_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn new_form_checks_enabled_channels() {
    let (app, pool) = common::test_app().await;
    let on = add_channel(&pool, "Ops", true).await;
    let off = add_channel(&pool, "Old", false).await;
    let r = get(&app, "/monitors/new").await;
    assert!(
        r.body.contains(&format!(r#"name="channel_{on}" checked"#)),
        "{}",
        r.body
    );
    assert!(r.body.contains(&format!(r#"name="channel_{off}"/>"#)));
}

#[tokio::test]
async fn monitor_form_links_checked_channels() {
    let (app, pool) = common::test_app().await;
    let a = add_channel(&pool, "A", true).await;
    let b = add_channel(&pool, "B", true).await;
    let r = post_form(
        &app,
        "/monitors/new",
        &form("API", &format!("&channel_{a}=on&channel_999=on")),
    )
    .await;
    assert_redirect(&r, "/monitors/1");
    assert_eq!(
        linked_channels(&pool, 1).await,
        vec![a],
        "unknown ids are ignored"
    );

    let r = get(&app, "/monitors/1/edit").await;
    assert!(r.body.contains(&format!(r#"name="channel_{a}" checked"#)));
    assert!(r.body.contains(&format!(r#"name="channel_{b}"/>"#)));

    let r = post_form(
        &app,
        "/monitors/1/edit",
        &form("API", &format!("&channel_{b}=on")),
    )
    .await;
    assert_redirect(&r, "/monitors/1");
    assert_eq!(linked_channels(&pool, 1).await, vec![b]);

    let r = post_form(
        &app,
        "/monitors/1/edit",
        &form("", &format!("&channel_{a}=on")),
    )
    .await;
    assert!(
        r.body.contains(&format!(r#"name="channel_{a}" checked"#)),
        "kept on error"
    );
}
