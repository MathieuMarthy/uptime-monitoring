//! The loop that checks one monitor, forever, at its own pace.

use crate::engine::http::HttpChecker;
use crate::engine::verdict::{self, Transition};
use crate::models::{Monitor, MonitorState};
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
            )
            .await
            {
                Ok(Some((next, transition))) => {
                    health = next;
                    log_transition(&monitor, transition, outcome.failure.as_ref());
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
