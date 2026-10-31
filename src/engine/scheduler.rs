//! The loop that checks one monitor, forever, at its own pace.

use crate::engine::http::HttpChecker;
use crate::engine::verdict::{self, CheckOutcome, ErrorKind, Failure, Health, Transition};
use crate::models::{Monitor, MonitorState, Recorded};
use crate::notify::{Event, MonitorRef, Notification, Notifier};
use crate::time::now_ms;
use crate::windows::Maintenances;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};
use tokio::time::Instant;

/// Pings of one push monitor closer than this to the last recorded ping are dropped.
const PING_SPACING_MS: i64 = 1000;

pub struct Scheduler {
    pub pool: SqlitePool,
    pub checker: HttpChecker,
    /// Caps checks running at the same time.
    pub permits: Semaphore,
    pub notifier: Notifier,
    pub maintenances: Maintenances,
}

impl Scheduler {
    /// Checks `monitor` until the task is aborted or the monitor is found paused or deleted.
    /// The next check starts `interval_s` (or `retry_interval_s` while failing below the
    /// threshold) after the start of the previous one.
    pub async fn run(self: Arc<Self>, monitor: Monitor, first_delay: Duration) {
        tokio::time::sleep(first_delay).await;
        let mut health = match MonitorState::find(&self.pool, monitor.id).await {
            Ok(Some(state)) => state.health(),
            Ok(None) => return,
            Err(e) => {
                tracing::error!(monitor_id = monitor.id, error = %e, "cannot read monitor state");
                return;
            }
        };

        loop {
            let started = Instant::now();
            let outcome = {
                let _permit = self
                    .permits
                    .acquire()
                    .await
                    .expect("semaphore never closed");
                self.checker.check(&monitor).await
            };

            match self.record(&monitor, &outcome, health).await {
                Some(next) => health = next,
                None => return,
            }

            let delay = verdict::next_delay_s(
                health,
                monitor.failure_threshold,
                monitor.interval_s,
                monitor.retry_interval_s,
            );
            tokio::time::sleep_until(started + Duration::from_secs(delay.unsigned_abs())).await;
        }
    }

    /// Waits for the pings of a push monitor, routed here by the supervisor. No ping within
    /// `interval_s + grace_s` of the last check is a missed check, and each further `interval_s`
    /// without one is another (a push monitor stores `retry_interval_s = interval_s`: one miss
    /// per missed run). Deadlines count from the last recorded check, so a restart or an edit
    /// does not postpone them; an overdue monitor still gets `grace_s` after the task starts.
    pub async fn run_push(
        self: Arc<Self>,
        monitor: Monitor,
        mut pings: mpsc::Receiver<CheckOutcome>,
    ) {
        let state = match MonitorState::find(&self.pool, monitor.id).await {
            Ok(Some(state)) => state,
            Ok(None) => return,
            Err(e) => {
                tracing::error!(monitor_id = monitor.id, error = %e, "cannot read monitor state");
                return;
            }
        };
        let mut health = state.health();
        let window_ms = (monitor.interval_s + monitor.grace_s) * 1000;
        let mut last_recorded = state.last_check_at.unwrap_or(0);
        let mut last_ping = i64::MIN;
        let mut due = state
            .last_check_at
            .map_or(now_ms() + window_ms, |last| last + window_ms)
            .max(now_ms() + monitor.grace_s * 1000);

        loop {
            let wait = Duration::from_millis((due - now_ms()).max(0).unsigned_abs());
            let mut outcome = tokio::select! {
                // A ping already queued wins over a deadline reached at the same time.
                biased;
                ping = pings.recv() => match ping {
                    // A burst records once per second.
                    Some(outcome) if outcome.checked_at < last_ping.saturating_add(PING_SPACING_MS) => continue,
                    Some(outcome) => outcome,
                    None => return,
                },
                () = tokio::time::sleep(wait) => CheckOutcome {
                    checked_at: now_ms(),
                    latency_ms: None,
                    status_code: None,
                    failure: Some(Failure {
                        kind: ErrorKind::Missed,
                        message: format!("no ping within {} s", window_ms / 1000),
                    }),
                    tls_expires_at: None,
                },
            };
            // Two checks of a monitor never share an instant (the primary key).
            outcome.checked_at = outcome.checked_at.max(last_recorded + 1);

            match self.record(&monitor, &outcome, health).await {
                Some(next) => health = next,
                None => return,
            }
            last_recorded = outcome.checked_at;
            due = if outcome
                .failure
                .as_ref()
                .is_some_and(|f| f.kind == ErrorKind::Missed)
            {
                let delay = verdict::next_delay_s(
                    health,
                    monitor.failure_threshold,
                    monitor.interval_s,
                    monitor.retry_interval_s,
                );
                outcome.checked_at + delay * 1000
            } else {
                last_ping = outcome.checked_at;
                last_ping + window_ms
            };
        }
    }

    /// Records one check and sends its alerts. Returns the new health, or `None` when the task
    /// must stop (see `MonitorState::record`). A database error keeps the old health.
    async fn record(
        &self,
        monitor: &Monitor,
        outcome: &CheckOutcome,
        health: Health,
    ) -> Option<Health> {
        match MonitorState::record(
            &self.pool,
            monitor.id,
            outcome,
            health,
            monitor.failure_threshold,
            monitor.tls_expiry_warn_days,
            self.maintenances.covers(monitor.id, outcome.checked_at),
        )
        .await
        {
            Ok(Some(recorded)) => {
                log_transition(monitor, recorded.transition, outcome.failure.as_ref());
                for event in events(health, &recorded, outcome) {
                    self.notifier.notify(Notification {
                        monitor: Some(MonitorRef {
                            id: monitor.id,
                            name: monitor.name.clone(),
                            url: (!monitor.is_push()).then(|| monitor.url.clone()),
                        }),
                        event,
                        at: outcome.checked_at,
                    });
                }
                Some(recorded.health)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::error!(monitor_id = monitor.id, error = %e, "cannot record check");
                Some(health)
            }
        }
    }
}

/// What to alert about after a check; `before` is the health the check started from.
fn events(before: Health, recorded: &Recorded, outcome: &CheckOutcome) -> Vec<Event> {
    let mut events = Vec::new();
    match recorded.transition {
        Some(Transition::WentDown { since }) => events.push(Event::Down {
            since,
            cause: outcome
                .failure
                .as_ref()
                .map(|f| f.message.clone())
                .unwrap_or_default(),
        }),
        Some(Transition::Recovered) => events.push(Event::Up {
            // A down monitor's series started when its incident did.
            down_since: before.failing_since.unwrap_or(outcome.checked_at),
        }),
        None => {}
    }
    if let Some(expires_at) = recorded.tls_warning {
        events.push(Event::TlsExpiring { expires_at });
    }
    events
}

fn log_transition(
    monitor: &Monitor,
    transition: Option<Transition>,
    failure: Option<&verdict::Failure>,
) {
    match transition {
        Some(Transition::WentDown { .. }) => tracing::warn!(
            monitor = %monitor.name,
            error = failure.map_or("", |f| f.message.as_str()),
            "monitor is down"
        ),
        Some(Transition::Recovered) => {
            tracing::info!(monitor = %monitor.name, "monitor is up again")
        }
        None => {}
    }
}
