mod common;

use sqlx::SqlitePool;
use uptime_monitoring::engine::verdict::{CheckOutcome, ErrorKind, Failure, Health, Transition};

const DAY_MS: i64 = 86_400_000;
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
        let recorded = MonitorState::record(pool, id, outcome, health, threshold, None)
            .await
            .unwrap()
            .expect("monitor is active");
        health = recorded.health;
        transitions.push(recorded.transition);
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
    let recorded = MonitorState::record(&pool, id, &ok_at(1_000), Health::PENDING, 1, None)
        .await
        .unwrap();
    assert_eq!(recorded, None);
    assert_eq!(count(&pool, "checks").await, 0);

    Monitor::delete(&pool, id).await.unwrap();
    let recorded = MonitorState::record(&pool, id, &ok_at(2_000), Health::PENDING, 1, None)
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

#[tokio::test]
async fn stale_health_is_not_recorded() {
    let (pool, id) = monitor("1").await;
    let stale = MonitorState::find(&pool, id)
        .await
        .unwrap()
        .unwrap()
        .health();
    record_all(&pool, id, 1, &[fail_at(1_000, ErrorKind::Timeout)]).await;
    // A pause and an unpause between two checks of a task that did not stop yet.
    Monitor::toggle_pause(&pool, id).await.unwrap();
    Monitor::toggle_pause(&pool, id).await.unwrap();
    let down = MonitorState::record(
        &pool,
        id,
        &fail_at(2_000, ErrorKind::Timeout),
        Health {
            status: MonitorStatus::Down,
            consecutive_failures: 1,
            failing_since: Some(1_000),
        },
        1,
        None,
    )
    .await
    .unwrap();
    assert_eq!(down, None, "the state was reset meanwhile");
    assert!(
        MonitorState::record(&pool, id, &ok_at(3_000), stale, 1, None)
            .await
            .unwrap()
            .is_some(),
        "pending is what the reset wrote"
    );
}

#[tokio::test]
async fn incident_opens_even_without_the_first_failed_check() {
    let (pool, id) = monitor("2").await;
    record_all(&pool, id, 2, &[fail_at(1_000, ErrorKind::Dns)]).await;
    // Purged by retention during a very long series.
    sqlx::query("DELETE FROM checks")
        .execute(&pool)
        .await
        .unwrap();
    let health = MonitorState::find(&pool, id)
        .await
        .unwrap()
        .unwrap()
        .health();
    MonitorState::record(
        &pool,
        id,
        &fail_at(2_000, ErrorKind::Timeout),
        health,
        2,
        None,
    )
    .await
    .unwrap();
    let incidents = Incident::for_monitor(&pool, id).await.unwrap();
    assert_eq!(incidents.len(), 1);
    assert_eq!(
        (incidents[0].started_at, incidents[0].error_kind.as_str()),
        (1_000, "timeout")
    );
}

#[tokio::test]
async fn response_without_tls_clears_the_expiry() {
    let (pool, id) = monitor("1").await;
    let plain = CheckOutcome {
        tls_expires_at: None,
        ..ok_at(2_000)
    };
    record_all(&pool, id, 1, &[ok_at(1_000), plain]).await;
    let s = MonitorState::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(s.tls_expires_at, None);
}

#[tokio::test]
async fn checks_record_the_time_they_cover_and_whether_it_was_downtime() {
    let (pool, id) = monitor("3").await; // checked every 60 s
    let s = 1000;
    let outcomes = [
        ok_at(0),
        fail_at(60 * s, ErrorKind::Timeout),
        fail_at(120 * s, ErrorKind::Timeout),
        fail_at(180 * s, ErrorKind::Timeout), // goes down, from the failure at 60 s
        ok_at(240 * s),                       // recovers
        ok_at(1000 * s),                      // after a gap longer than twice the interval
    ];
    record_all(&pool, id, 3, &outcomes).await;

    let rows: Vec<(i64, i64, bool)> =
        sqlx::query_as("SELECT checked_at, covered_ms, down FROM checks ORDER BY checked_at")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rows,
        [
            (0, 0, false),
            (60 * s, 60 * s, false),
            (120 * s, 60 * s, true),
            (180 * s, 60 * s, true),
            (240 * s, 60 * s, true),
            (1000 * s, 0, false),
        ]
    );
}

fn ok_with_cert(at: i64, expires_at: i64) -> CheckOutcome {
    CheckOutcome {
        tls_expires_at: Some(expires_at),
        ..ok_at(at)
    }
}

/// Records one check with a 14-day certificate warning and returns the expiry it warns about.
async fn record_cert(pool: &SqlitePool, id: i64, outcome: &CheckOutcome) -> Option<i64> {
    let health = MonitorState::find(pool, id)
        .await
        .unwrap()
        .unwrap()
        .health();
    MonitorState::record(pool, id, outcome, health, 1, Some(14))
        .await
        .unwrap()
        .expect("monitor is active")
        .tls_warning
}

#[tokio::test]
async fn certificate_warning_is_given_once_per_expiry_date() {
    let (pool, id) = monitor("1").await;
    let soon = 20 * DAY_MS;
    assert_eq!(
        record_cert(&pool, id, &ok_with_cert(0, soon)).await,
        None,
        "20 days left"
    );
    assert_eq!(
        record_cert(&pool, id, &ok_with_cert(7 * DAY_MS, soon)).await,
        Some(soon),
        "13 days left"
    );
    assert_eq!(
        record_cert(&pool, id, &ok_with_cert(8 * DAY_MS, soon)).await,
        None,
        "already warned"
    );
    let renewed = 98 * DAY_MS;
    assert_eq!(
        record_cert(&pool, id, &ok_with_cert(9 * DAY_MS, renewed)).await,
        None
    );
    assert_eq!(
        record_cert(&pool, id, &ok_with_cert(85 * DAY_MS, renewed)).await,
        Some(renewed),
        "the renewed certificate expires soon in turn"
    );
}

#[tokio::test]
async fn certificate_warning_needs_a_threshold_and_a_certificate() {
    let (pool, id) = monitor("1").await;
    let health = MonitorState::find(&pool, id)
        .await
        .unwrap()
        .unwrap()
        .health();
    let recorded = MonitorState::record(&pool, id, &ok_with_cert(0, DAY_MS), health, 1, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recorded.tls_warning, None, "warning disabled");
    assert_eq!(
        record_cert(&pool, id, &fail_at(1_000, ErrorKind::Timeout)).await,
        Some(DAY_MS),
        "a check without a response keeps the last certificate"
    );
    let plain = CheckOutcome {
        tls_expires_at: None,
        ..ok_at(2_000)
    };
    assert_eq!(record_cert(&pool, id, &plain).await, None, "no TLS");
}
