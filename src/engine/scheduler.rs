//! The loop that checks one monitor, forever, at its own pace.

use crate::engine::http::HttpChecker;
use crate::engine::verdict::{self, CheckOutcome, Health, Transition};
use crate::models::{Monitor, MonitorState, Recorded};
use crate::notify::{Event, MonitorRef, Notification, Notifier};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::Instant;

pub struct Scheduler {
    pub pool: SqlitePool,
    pub checker: HttpChecker,
    /// Caps checks running at the same time.
    pub permits: Semaphore,
    pub notifier: Notifier,
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

            match MonitorState::record(
                &self.pool,
                monitor.id,
                &outcome,
                health,
                monitor.failure_threshold,
                monitor.tls_expiry_warn_days,
            )
            .await
            {
                Ok(Some(recorded)) => {
                    log_transition(&monitor, recorded.transition, outcome.failure.as_ref());
                    for event in events(health, &recorded, &outcome) {
                        self.notifier.notify(Notification {
                            monitor: Some(MonitorRef {
                                id: monitor.id,
                                name: monitor.name.clone(),
                                url: monitor.url.clone(),
                            }),
                            event,
                            at: outcome.checked_at,
                        });
                    }
                    health = recorded.health;
                }
                Ok(None) => return,
                Err(e) => {
                    tracing::error!(monitor_id = monitor.id, error = %e, "cannot record check");
                }
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
