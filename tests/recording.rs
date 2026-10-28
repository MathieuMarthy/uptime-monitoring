mod common;

use sqlx::SqlitePool;
use uptime_monitoring::engine::verdict::{CheckOutcome, ErrorKind, Failure, Health, Transition};
use uptime_monitoring::forms::monitor::{MonitorForm, MonitorInput};
use uptime_monitoring::models::{Incident, Monitor, MonitorState, MonitorStatus};

fn input(threshold: &str) -> MonitorInput {
    MonitorForm {
        name: "API".to_string(),
        url: "https://example.com".to_string(),
        failure_threshold: threshold.to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap()
}

async fn monitor(threshold: &str) -> (SqlitePool, i64) {
    let pool = common::test_app().await.1;
    let id = Monitor::create(&pool, &input(threshold)).await.unwrap();
    (pool, id)
}

fn ok_at(at: i64) -> CheckOutcome {
    CheckOutcome {
        checked_at: at,
        latency_ms: Some(42),
        status_code: Some(200),
        failure: None,
        tls_expires_at: Some(9_000),
    }
}

fn fail_at(at: i64, kind: ErrorKind) -> CheckOutcome {
    CheckOutcome {
        checked_at: at,
        latency_ms: None,
        status_code: None,
        failure: Some(Failure {
            kind,
            message: format!("{} at {at}", kind.as_str()),
        }),
        tls_expires_at: None,
    }
}

/// Records each outcome in turn, as the scheduler does, and returns the transitions.
async fn record_all(
    pool: &SqlitePool,
    id: i64,
    threshold: i64,
    outcomes: &[CheckOutcome],
) -> Vec<Option<Transition>> {
    let mut health = MonitorState::find(pool, id)
        .await
        .unwrap()
        .unwrap()
        .health();
    let mut transitions = Vec::new();
    for outcome in outcomes {
        let (next, transition) = MonitorState::record(pool, id, outcome, health, threshold)
            .await
            .unwrap()
            .expect("monitor is active");
        health = next;
        transitions.push(transition);
    }
    transitions
}

async fn count(pool: &SqlitePool, table: &str) -> i64 {
    sqlx::query_as::<_, (i64,)>(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
        .0
}

#[tokio::test]
async fn successful_check_is_recorded() {
    let (pool, id) = monitor("3").await;
    record_all(&pool, id, 3, &[ok_at(1_000)]).await;

    let s = MonitorState::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(s.status, MonitorStatus::Up);
    assert_eq!(
        (
            s.last_check_at,
            s.last_latency_ms,
            s.last_error,
            s.tls_expires_at
        ),
        (Some(1_000), Some(42), None, Some(9_000))
    );
    let row: (i64, i64, Option<i64>, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT checked_at, ok, latency_ms, status_code, error_kind FROM checks WHERE monitor_id = ?",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (1_000, 1, Some(42), Some(200), None));
}

#[tokio::test]
async fn reaching_threshold_opens_incident_from_first_failure() {
    let (pool, id) = monitor("2").await;
    let transitions = record_all(
        &pool,
        id,
        2,
        &[
            ok_at(1_000),
            fail_at(2_000, ErrorKind::Timeout),
            fail_at(3_000, ErrorKind::Connect),
        ],
    )
    .await;
    assert_eq!(
        transitions,
        vec![None, None, Some(Transition::WentDown { since: 2_000 })]
    );

    let incidents = Incident::for_monitor(&pool, id).await.unwrap();
    assert_eq!(incidents.len(), 1);
    let i = &incidents[0];
    assert_eq!((i.started_at, i.resolved_at), (2_000, None));
    assert_eq!(i.error_kind, "timeout");
    assert_eq!(i.cause.as_deref(), Some("timeout at 2000"));

    let s = MonitorState::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(s.status, MonitorStatus::Down);
    assert_eq!(s.last_error.as_deref(), Some("connect at 3000"));
    assert_eq!(
        s.tls_expires_at,
        Some(9_000),
        "kept when no certificate was seen"
    );
}

#[tokio::test]
async fn threshold_one_opens_incident_on_the_same_check() {
    let (pool, id) = monitor("1").await;
    record_all(&pool, id, 1, &[fail_at(5_000, ErrorKind::Dns)]).await;
    let incidents = Incident::for_monitor(&pool, id).await.unwrap();
    assert_eq!(
        (incidents[0].started_at, incidents[0].error_kind.as_str()),
        (5_000, "dns")
    );
}

#[tokio::test]
async fn recovery_resolves_incident() {
    let (pool, id) = monitor("1").await;
    let transitions = record_all(
        &pool,
        id,
        1,
        &[fail_at(1_000, ErrorKind::Status), ok_at(4_000)],
    )
    .await;
    assert_eq!(transitions[1], Some(Transition::Recovered));
    let incidents = Incident::for_monitor(&pool, id).await.unwrap();
    assert_eq!(incidents[0].resolved_at, Some(4_000));
}

#[tokio::test]
async fn paused_or_deleted_monitor_is_not_recorded() {
    let (pool, id) = monitor("1").await;
    Monitor::toggle_pause(&pool, id).await.unwrap();
    let recorded = MonitorState::record(&pool, id, &ok_at(1_000), Health::PENDING, 1)
        .await
        .unwrap();
    assert_eq!(recorded, None);
    assert_eq!(count(&pool, "checks").await, 0);

    Monitor::delete(&pool, id).await.unwrap();
    let recorded = MonitorState::record(&pool, id, &ok_at(2_000), Health::PENDING, 1)
        .await
        .unwrap();
    assert_eq!(recorded, None);
    assert_eq!(count(&pool, "checks").await, 0);
}

async fn assert_paused_and_reset(pool: &SqlitePool, id: i64) {
    let incidents = Incident::for_monitor(pool, id).await.unwrap();
    assert!(
        incidents[0].resolved_at.is_some(),
        "incident closed on pause"
    );
    let s = MonitorState::find(pool, id).await.unwrap().unwrap();
    assert_eq!(s.health(), Health::PENDING);
}

#[tokio::test]
async fn toggle_pause_closes_incident_and_resets_state() {
    let (pool, id) = monitor("1").await;
    record_all(&pool, id, 1, &[fail_at(1_000, ErrorKind::Timeout)]).await;
    Monitor::toggle_pause(&pool, id).await.unwrap();
    assert_paused_and_reset(&pool, id).await;
}

#[tokio::test]
async fn pausing_through_the_form_closes_incident_and_resets_state() {
    let (pool, id) = monitor("1").await;
    record_all(&pool, id, 1, &[fail_at(1_000, ErrorKind::Timeout)]).await;
    let paused = MonitorInput {
        paused: true,
        ..input("1")
    };
    Monitor::update(&pool, id, &paused).await.unwrap();
    assert_paused_and_reset(&pool, id).await;
}

#[tokio::test]
async fn editing_an_active_monitor_keeps_its_state() {
    let (pool, id) = monitor("1").await;
    record_all(&pool, id, 1, &[fail_at(1_000, ErrorKind::Timeout)]).await;
    Monitor::update(&pool, id, &input("2")).await.unwrap();
    let s = MonitorState::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(s.status, MonitorStatus::Down);
    assert_eq!(
        Incident::for_monitor(&pool, id).await.unwrap()[0].resolved_at,
        None
    );
}
