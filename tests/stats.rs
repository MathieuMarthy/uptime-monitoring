mod common;

use sqlx::SqlitePool;
use uptime_monitoring::forms::monitor::MonitorForm;
use uptime_monitoring::models::{Bucket, Monitor};
use uptime_monitoring::time::{day_key, day_of};
use uptime_monitoring::views::heartbeat::DailyHistory;

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
    let mut buckets = Bucket::query(pool, Some(&[monitor_id]), T0, HOUR, T0 + HOUR)
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
    let buckets = Bucket::query(&pool, Some(&[id]), T0, 60 * S, T0 + 180 * S)
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

    let c = monitor(&pool, "Third").await;
    check(&pool, c, T0, true, false, None).await;
    let some = Bucket::query(&pool, Some(&[a, c]), T0, HOUR, T0 + HOUR)
        .await
        .unwrap();
    assert_eq!(
        some.iter().map(|b| b.monitor_id).collect::<Vec<_>>(),
        [a, c]
    );
}

#[tokio::test]
async fn daily_history_of_several_monitors() {
    let (pool, a) = setup().await;
    let b = monitor(&pool, "Other").await;
    let tz = jiff::tz::TimeZone::UTC;
    let yesterday = day_key(day_of(&tz, T0 - 24 * HOUR));
    sqlx::query(
        "INSERT INTO daily_stats (monitor_id, day, monitored_ms, downtime_ms, checks_total,
                                  checks_failed)
         VALUES (?, ?, 3600000, 360000, 60, 6)",
    )
    .bind(a)
    .bind(yesterday)
    .execute(&pool)
    .await
    .unwrap();
    check(&pool, a, T0 - 60 * S, true, false, None).await;

    let many = DailyHistory::load_many(&pool, &[a, b], &tz, T0)
        .await
        .unwrap();
    let ha = &many[&a];
    let classes: Vec<_> = ha.bars.ticks[87..].iter().map(|t| t.class).collect();
    assert_eq!(classes, ["tick-none", "tick-down", "tick-up"]);
    assert_eq!(ha.uptime_90d, "90.16%");
    assert_eq!(ha.uptime_30d, ha.uptime_90d);
    assert!(many[&b].bars.ticks.iter().all(|t| t.class == "tick-none"));
    assert_eq!(many[&b].uptime_90d, "—");

    let one = DailyHistory::load(&pool, a, &tz, T0).await.unwrap();
    assert_eq!(one.uptime_90d, ha.uptime_90d);
    let titles = |h: &DailyHistory| {
        h.bars
            .ticks
            .iter()
            .map(|t| t.title.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(titles(&one), titles(ha));
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
