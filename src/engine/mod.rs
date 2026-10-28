//! The check engine: runs the checks, records them and drives the monitor state machine.
//!
//! A supervisor task owns one scheduler task per active monitor. Routes send `reload(id)` after
//! any change to a monitor; the supervisor then restarts or stops that monitor's task.

pub mod guard;
pub mod http;
pub mod scheduler;
pub mod verdict;

use crate::engine::guard::Guard;
use crate::engine::http::HttpChecker;
use crate::engine::scheduler::Scheduler;
use crate::models::{Monitor, Settings};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;

/// Checks running at the same time, across all monitors.
const MAX_CONCURRENT_CHECKS: usize = 20;
/// At startup, first checks are spread over the interval, at most this long.
const MAX_STARTUP_SPREAD: Duration = Duration::from_secs(60);

/// Sends commands to the running engine. The default handle is disconnected and ignores them,
/// which is what tests of the web layer use.
#[derive(Clone, Default)]
pub struct EngineHandle {
    commands: Option<mpsc::UnboundedSender<i64>>,
}

impl EngineHandle {
    /// Re-reads the monitor and restarts its checks, or stops them if it is paused or deleted.
    pub fn reload(&self, monitor_id: i64) {
        if let Some(commands) = &self.commands {
            // Fails only once the engine has stopped, during shutdown.
            let _ = commands.send(monitor_id);
        }
    }
}

/// Starts the engine: one task per active monitor, first checks spread at random.
pub async fn start(pool: SqlitePool) -> Result<EngineHandle, sqlx::Error> {
    let guard = Guard::new(Settings::block_private_targets(&pool).await?);
    let scheduler = Arc::new(Scheduler {
        pool: pool.clone(),
        checker: HttpChecker::new(guard),
        permits: Semaphore::new(MAX_CONCURRENT_CHECKS),
    });
    let mut supervisor = Supervisor {
        pool,
        scheduler,
        tasks: HashMap::new(),
    };

    for monitor in Monitor::find_all(&supervisor.pool).await? {
        if !monitor.paused {
            let spread =
                Duration::from_secs(monitor.interval_s.unsigned_abs()).min(MAX_STARTUP_SPREAD);
            supervisor.spawn(monitor, random_below(spread));
        }
    }

    let (commands, receiver) = mpsc::unbounded_channel();
    tokio::spawn(supervisor.run(receiver));
    Ok(EngineHandle {
        commands: Some(commands),
    })
}

struct Supervisor {
    pool: SqlitePool,
    scheduler: Arc<Scheduler>,
    tasks: HashMap<i64, JoinHandle<()>>,
}

impl Supervisor {
    async fn run(mut self, mut receiver: mpsc::UnboundedReceiver<i64>) {
        while let Some(monitor_id) = receiver.recv().await {
            self.reload(monitor_id).await;
        }
    }

    async fn reload(&mut self, monitor_id: i64) {
        if let Some(task) = self.tasks.remove(&monitor_id) {
            task.abort();
            // Waits for the task to stop, so it cannot record a check after the new one reads
            // the state.
            let _ = task.await;
        }
        match Monitor::find(&self.pool, monitor_id).await {
            Ok(Some(monitor)) if !monitor.paused => self.spawn(monitor, Duration::ZERO),
            Ok(_) => {}
            Err(e) => tracing::error!(monitor_id, error = %e, "cannot reload monitor"),
        }
    }

    fn spawn(&mut self, monitor: Monitor, first_delay: Duration) {
        let id = monitor.id;
        let task = tokio::spawn(self.scheduler.clone().run(monitor, first_delay));
        self.tasks.insert(id, task);
    }
}

fn random_below(limit: Duration) -> Duration {
    let millis = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX).max(1);
    Duration::from_millis(getrandom::u64().unwrap_or(0) % millis)
}
