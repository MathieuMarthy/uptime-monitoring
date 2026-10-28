mod common;

use sqlx::SqlitePool;
use uptime_monitoring::forms::monitor::MonitorForm;
use uptime_monitoring::models::{Bucket, Monitor};

const S: i64 = 1000;
const T0: i64 = 1_800_000_000_000;
const HOUR: i64 = 3600 * S;

async fn setup() -> (SqlitePool, i64) {
    let pool = common::test_app().await.1;
    let id = monitor(&pool, "API").await;
    (pool, id)
}

async fn monitor(pool: &SqlitePool, name: &str) -> i64 {
    let input = MonitorForm {
        name: name.to_string(),
        url: "https://example.com".to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap();
    Monitor::create(pool, &input).await.unwrap()
}

/// A check covering 60 s, ok or down as told.
async fn check(
    pool: &SqlitePool,
    monitor_id: i64,
    at: i64,
    ok: bool,
    down: bool,
    latency: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO checks (monitor_id, checked_at, ok, latency_ms, covered_ms, down)
         VALUES (?, ?, ?, ?, 60000, ?)",
    )
    .bind(monitor_id)
    .bind(at)
    .bind(ok)
    .bind(latency)
    .bind(down)
    .execute(pool)
    .await
    .unwrap();
}

async fn hour(pool: &SqlitePool, monitor_id: i64) -> Bucket {
    let mut buckets = Bucket::query(pool, Some(monitor_id), T0, HOUR, T0 + HOUR)
        .await
        .unwrap();
    assert_eq!(buckets.len(), 1);
    buckets.remove(0)
}

#[tokio::test]
async fn sums_the_checks_of_a_bucket() {
    let (pool, id) = setup().await;
    check(&pool, id, T0, true, false, Some(100)).await;
    check(&pool, id, T0 + 60 * S, false, false, None).await;
    check(&pool, id, T0 + 120 * S, false, true, None).await;
    check(&pool, id, T0 + 180 * S, true, true, Some(301)).await;

    let b = hour(&pool, id).await;
    assert_eq!((b.monitor_id, b.index), (id, 0));
    assert_eq!((b.monitored_ms, b.downtime_ms), (240 * S, 120 * S));
    assert_eq!((b.checks_total, b.checks_failed), (4, 2));
    assert_eq!((b.latency_avg_ms, b.latency_count), (Some(201), 2));
    assert_eq!(b.uptime(), Some(0.5));
}

#[tokio::test]
async fn buckets_split_the_window_and_skip_empty_ones() {
    let (pool, id) = setup().await;
    check(&pool, id, T0 - S, true, false, None).await; // before the window
    check(&pool, id, T0, true, false, None).await;
    check(&pool, id, T0 + 59 * S, true, false, None).await;
    check(&pool, id, T0 + 150 * S, true, false, None).await;
    check(&pool, id, T0 + 180 * S, true, false, None).await; // at `to`, excluded
    let buckets = Bucket::query(&pool, Some(id), T0, 60 * S, T0 + 180 * S)
        .await
        .unwrap();
    let got: Vec<_> = buckets.iter().map(|b| (b.index, b.checks_total)).collect();
    assert_eq!(got, [(0, 2), (2, 1)]);
}

#[tokio::test]
async fn all_monitors_or_one() {
    let (pool, a) = setup().await;
    let b = monitor(&pool, "Other").await;
    check(&pool, a, T0, true, false, None).await;
    check(&pool, b, T0, true, false, None).await;
    let all = Bucket::query(&pool, None, T0, HOUR, T0 + HOUR)
        .await
        .unwrap();
    assert_eq!(all.iter().map(|b| b.monitor_id).collect::<Vec<_>>(), [a, b]);
    assert_eq!(hour(&pool, b).await.monitor_id, b);
}

#[tokio::test]
async fn no_coverage_means_no_uptime() {
    let (pool, id) = setup().await;
    sqlx::query("INSERT INTO checks (monitor_id, checked_at, ok) VALUES (?, ?, 1)")
        .bind(id)
        .bind(T0)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(hour(&pool, id).await.uptime(), None);
}
