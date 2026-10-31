mod common;

use sqlx::SqlitePool;
use uptime_monitoring::forms::monitor::MonitorForm;
use uptime_monitoring::maintenance;
use uptime_monitoring::models::Monitor;

const S: i64 = 1000;
const HOUR: i64 = 3600 * S;
const DAY: i64 = 24 * HOUR;
/// 2026-10-26T00:00:00Z, a Monday.
const D0: i64 = 1_792_972_800_000;

async fn setup() -> (SqlitePool, i64) {
    let pool = common::test_app().await.1;
    let input = MonitorForm {
        name: "API".to_string(),
        url: "https://example.com".to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap();
    let id = Monitor::create(&pool, &input).await.unwrap();
    (pool, id)
}

async fn check(pool: &SqlitePool, monitor_id: i64, at: i64, down: bool, latency: Option<i64>) {
    sqlx::query(
        "INSERT INTO checks (monitor_id, checked_at, ok, latency_ms, covered_ms, down)
         VALUES (?, ?, ?, ?, 60000, ?)",
    )
    .bind(monitor_id)
    .bind(at)
    .bind(!down)
    .bind(latency)
    .bind(down)
    .execute(pool)
    .await
    .unwrap();
}

type Row = (i64, i64, i64, i64, i64, Option<i64>, Option<i64>);

async fn daily_stats(pool: &SqlitePool) -> Vec<Row> {
    sqlx::query_as(
        "SELECT day, monitored_ms, downtime_ms, checks_total, checks_failed, latency_avg_ms,
                latency_p95_ms
         FROM daily_stats ORDER BY day",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn count(pool: &SqlitePool, table: &str) -> i64 {
    sqlx::query_as::<_, (i64,)>(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
        .0
}

#[tokio::test]
async fn completed_days_are_rolled_up_once() {
    let (pool, id) = setup().await;
    // 20 checks on 2026-10-26, latencies 10..=200 ms, the last two down.
    for i in 0..20 {
        check(&pool, id, D0 + i * 60 * S, i >= 18, Some((i + 1) * 10)).await;
    }
    check(&pool, id, D0 + DAY + HOUR, false, Some(5)).await; // today

    let now = D0 + DAY + 2 * HOUR;
    maintenance::run(&pool, now).await.unwrap();
    let expected = vec![(
        20261026,
        20 * 60 * S,
        2 * 60 * S,
        20,
        2,
        Some(105),
        Some(190),
    )];
    assert_eq!(daily_stats(&pool).await, expected);

    // A later run neither duplicates nor rewrites the day.
    sqlx::query("DELETE FROM checks WHERE checked_at < ?")
        .bind(D0 + 10 * 60 * S)
        .execute(&pool)
        .await
        .unwrap();
    maintenance::run(&pool, now + HOUR).await.unwrap();
    assert_eq!(daily_stats(&pool).await, expected);
}

#[tokio::test]
async fn a_day_is_rolled_up_a_few_minutes_after_it_ends() {
    let (pool, id) = setup().await;
    check(&pool, id, D0 + HOUR, false, None).await;
    maintenance::run(&pool, D0 + DAY + 60 * S).await.unwrap();
    assert!(daily_stats(&pool).await.is_empty());
    maintenance::run(&pool, D0 + DAY + 10 * 60 * S)
        .await
        .unwrap();
    assert_eq!(daily_stats(&pool).await.len(), 1);
}

#[tokio::test]
async fn days_are_those_of_the_instance_timezone() {
    let (pool, id) = setup().await;
    sqlx::query("UPDATE settings SET timezone = 'Europe/Paris'")
        .execute(&pool)
        .await
        .unwrap();
    check(&pool, id, D0 - 30 * 60 * S, false, None).await; // 00:30 on 10-26 in Paris
    check(&pool, id, D0 + 23 * HOUR + 30 * 60 * S, false, None).await; // 00:30 on 10-27
    maintenance::run(&pool, D0 + 2 * DAY).await.unwrap();
    let days: Vec<i64> = daily_stats(&pool).await.iter().map(|r| r.0).collect();
    assert_eq!(days, [20261026, 20261027]);
}

#[tokio::test]
async fn raw_checks_are_purged_after_their_retention() {
    let (pool, id) = setup().await;
    check(&pool, id, D0, false, None).await;
    check(&pool, id, D0 + 7 * DAY + HOUR, false, None).await;
    // Default retention: 7 days. The old check is rolled up before it goes.
    maintenance::run(&pool, D0 + 8 * DAY).await.unwrap();
    assert_eq!(count(&pool, "checks").await, 1);
    assert_eq!(daily_stats(&pool).await[0].0, 20261026);
}

#[tokio::test]
async fn history_is_kept_unless_a_retention_is_set() {
    let (pool, id) = setup().await;
    check(&pool, id, D0, false, None).await;
    sqlx::query(
        "INSERT INTO incidents (monitor_id, started_at, resolved_at, error_kind)
         VALUES (?1, ?2, ?2 + 1000, 'status'), (?1, ?2 + 5000, NULL, 'status')",
    )
    .bind(id)
    .bind(D0)
    .execute(&pool)
    .await
    .unwrap();
    let later = D0 + 100 * DAY;
    maintenance::run(&pool, later).await.unwrap();
    assert_eq!(
        (
            count(&pool, "incidents").await,
            count(&pool, "daily_stats").await
        ),
        (2, 1)
    );

    sqlx::query("UPDATE settings SET history_retention_days = 90")
        .execute(&pool)
        .await
        .unwrap();
    maintenance::run(&pool, later).await.unwrap();
    // The open incident stays, however old.
    assert_eq!(
        (
            count(&pool, "incidents").await,
            count(&pool, "daily_stats").await
        ),
        (1, 0)
    );
}

#[tokio::test]
async fn expired_sessions_are_purged() {
    let (pool, _) = setup().await;
    sqlx::query("UPDATE sessions SET expires_at = 0")
        .execute(&pool)
        .await
        .unwrap();
    maintenance::run(&pool, D0).await.unwrap();
    assert_eq!(count(&pool, "sessions").await, 0);
}

async fn configure(pool: &SqlitePool, threshold: i64, interval_s: i64) {
    sqlx::query(
        "UPDATE monitors SET failure_threshold = ?, interval_s = ?2, retry_interval_s = ?2",
    )
    .bind(threshold)
    .bind(interval_s)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn rollup_waits_for_the_longest_possible_failure_series() {
    // 3 failures 300 s apart (10 s timeout): an incident can open 930 s after its first failure,
    // and mark checks of the previous day as downtime.
    let (pool, id) = setup().await;
    configure(&pool, 3, 300).await;
    check(&pool, id, D0 + DAY - 120 * S, true, None).await;
    maintenance::run(&pool, D0 + DAY + 10 * 60 * S)
        .await
        .unwrap();
    assert!(daily_stats(&pool).await.is_empty());
    maintenance::run(&pool, D0 + DAY + 25 * 60 * S)
        .await
        .unwrap();
    assert_eq!(daily_stats(&pool).await.len(), 1);
}

#[tokio::test]
async fn checks_of_days_not_rolled_up_yet_are_not_purged() {
    let (pool, id) = setup().await;
    configure(&pool, 10, 86_400).await; // a failure series can last 10 days
    check(&pool, id, D0, false, None).await;
    maintenance::run(&pool, D0 + 9 * DAY).await.unwrap();
    assert_eq!(count(&pool, "checks").await, 1);
    assert!(daily_stats(&pool).await.is_empty());
    maintenance::run(&pool, D0 + 12 * DAY).await.unwrap();
    assert_eq!(
        (count(&pool, "checks").await, daily_stats(&pool).await.len()),
        (0, 1)
    );
}

#[tokio::test]
async fn p95_is_the_nearest_rank() {
    for (n, expected) in [(1, 10), (19, 190), (20, 190), (21, 200)] {
        let (pool, id) = setup().await;
        for i in 0..n {
            check(&pool, id, D0 + i * 60 * S, false, Some((i + 1) * 10)).await;
        }
        maintenance::run(&pool, D0 + DAY + HOUR).await.unwrap();
        assert_eq!(
            daily_stats(&pool).await[0].6,
            Some(expected),
            "{n} latencies"
        );
    }
}

#[tokio::test]
async fn a_dst_day_rolls_up_its_25_hours() {
    let (pool, id) = setup().await;
    sqlx::query("UPDATE settings SET timezone = 'Europe/Paris'")
        .execute(&pool)
        .await
        .unwrap();
    // 2026-10-25 in Paris runs from 10-24T22:00Z to 10-25T23:00Z.
    let start = D0 - DAY - 2 * HOUR;
    check(&pool, id, start, false, None).await;
    check(&pool, id, start + 25 * HOUR - S, false, None).await;
    check(&pool, id, start + 25 * HOUR, false, None).await;
    maintenance::run(&pool, D0 + DAY).await.unwrap();
    let rows: Vec<(i64, i64)> = daily_stats(&pool)
        .await
        .iter()
        .map(|r| (r.0, r.3))
        .collect();
    assert_eq!(rows, [(20261025, 2), (20261026, 1)]);
}

#[tokio::test]
async fn ended_maintenance_windows_follow_the_history_retention() {
    use uptime_monitoring::models::{Maintenance, MaintenanceInput};
    use uptime_monitoring::windows::Schedule;
    let (pool, _) = setup().await;
    let window = |ends_at| MaintenanceInput {
        title: "Deploy".to_string(),
        schedule: Schedule::Once {
            starts_at: ends_at - HOUR,
            ends_at,
        },
        all_monitors: true,
        show_on_status_pages: false,
        monitor_ids: vec![],
    };
    let old = Maintenance::create(&pool, &window(D0)).await.unwrap();
    let recent = Maintenance::create(&pool, &window(D0 + 20 * DAY))
        .await
        .unwrap();
    maintenance::run(&pool, D0 + 40 * DAY).await.unwrap();
    assert!(
        Maintenance::find(&pool, old).await.unwrap().is_some(),
        "kept forever by default"
    );

    sqlx::query("UPDATE settings SET history_retention_days = 30")
        .execute(&pool)
        .await
        .unwrap();
    maintenance::run(&pool, D0 + 40 * DAY).await.unwrap();
    assert!(Maintenance::find(&pool, old).await.unwrap().is_none());
    assert!(Maintenance::find(&pool, recent).await.unwrap().is_some());
}
