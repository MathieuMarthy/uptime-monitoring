//! The check engine: runs the checks, records them and drives the monitor state machine.
//!
//! A supervisor task owns one scheduler task per active monitor. Routes send `reload(id)` after
//! any change to a monitor; the supervisor then restarts or stops that monitor's task. Pings of
//! push monitors go through the supervisor too, to the task of their monitor.

pub mod guard;
pub mod http;
pub mod scheduler;
pub mod verdict;

use crate::engine::guard::Guard;
use crate::engine::http::HttpChecker;
use crate::engine::scheduler::Scheduler;
use crate::engine::verdict::CheckOutcome;
use crate::models::Monitor;
use crate::notify::Notifier;
use crate::windows::Maintenances;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;

/// Checks running at the same time, across all monitors.
const MAX_CONCURRENT_CHECKS: usize = 20;
/// Pings waiting for the supervisor, across monitors, and for one push monitor's task. Beyond,
/// pings are dropped: a flood cannot grow memory.
const PING_QUEUE: usize = 256;
const MONITOR_PING_QUEUE: usize = 4;
/// At startup, first checks are spread over the interval, at most this long.
const MAX_STARTUP_SPREAD: Duration = Duration::from_secs(60);

/// Sends commands to the running engine. The default handle is disconnected and ignores them,
/// which is what tests of the web layer use.
#[derive(Clone, Default)]
pub struct EngineHandle {
    commands: Option<mpsc::UnboundedSender<Reload>>,
    pings: Option<mpsc::Sender<(i64, CheckOutcome)>>,
}

impl EngineHandle {
    /// Re-reads the monitor and restarts its checks, or stops them if it is paused or deleted.
    pub fn reload(&self, monitor_id: i64) {
        self.send(Reload {
            monitor_id,
            slot: 0,
            slots: 1,
        });
    }

    /// Starts monitors added together with their first checks spread evenly over the interval
    /// (at most a minute), the first at once: they do not all check in the same instant, again
    /// and again.
    pub fn reload_staggered(&self, monitor_ids: &[i64]) {
        let slots = u32::try_from(monitor_ids.len()).unwrap_or(u32::MAX);
        for (slot, &monitor_id) in (0..slots).zip(monitor_ids) {
            self.send(Reload {
                monitor_id,
                slot,
                slots,
            });
        }
    }

    fn send(&self, reload: Reload) {
        if let Some(commands) = &self.commands {
            // Fails only once the engine has stopped, during shutdown.
            let _ = commands.send(reload);
        }
    }

    /// Hands a ping (a success, or a failure the job reported) to a push monitor's task. Dropped
    /// when the queue is full or the monitor has no task (paused, deleted, not a push monitor).
    pub fn ping(&self, monitor_id: i64, outcome: CheckOutcome) {
        if let Some(pings) = &self.pings {
            let _ = pings.try_send((monitor_id, outcome));
        }
    }
}

/// Restart `monitor_id`, its first check delayed by `slot / slots` of its spread.
struct Reload {
    monitor_id: i64,
    slot: u32,
    slots: u32,
}

/// Starts the engine: one task per active monitor, first checks spread at random. Alerts go to
/// `notifier`; checks made during a window of `maintenances` neither count nor alert.
pub async fn start(
    pool: SqlitePool,
    guard: Guard,
    notifier: Notifier,
    maintenances: Maintenances,
) -> Result<EngineHandle, sqlx::Error> {
    let scheduler = Arc::new(Scheduler {
        pool: pool.clone(),
        checker: HttpChecker::new(guard),
        permits: Semaphore::new(MAX_CONCURRENT_CHECKS),
        notifier,
        maintenances,
    });
    let mut supervisor = Supervisor {
        pool,
        scheduler,
        tasks: HashMap::new(),
    };

    for monitor in Monitor::find_all(&supervisor.pool).await? {
        if !monitor.paused {
            let spread = startup_spread(&monitor);
            supervisor.spawn(monitor, random_below(spread));
        }
    }

    let (commands, receiver) = mpsc::unbounded_channel();
    let (pings, ping_receiver) = mpsc::channel(PING_QUEUE);
    tokio::spawn(supervisor.run(receiver, ping_receiver));
    Ok(EngineHandle {
        commands: Some(commands),
        pings: Some(pings),
    })
}

struct Supervisor {
    pool: SqlitePool,
    scheduler: Arc<Scheduler>,
    tasks: HashMap<i64, Task>,
}

struct Task {
    handle: JoinHandle<()>,
    /// Push monitors only.
    pings: Option<mpsc::Sender<CheckOutcome>>,
}

impl Supervisor {
    async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Reload>,
        mut pings: mpsc::Receiver<(i64, CheckOutcome)>,
    ) {
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(reload) => self.reload(reload).await,
                    None => return,
                },
                Some((monitor_id, outcome)) = pings.recv() => {
                    if let Some(sender) = self.tasks.get(&monitor_id).and_then(|t| t.pings.as_ref()) {
                        let _ = sender.try_send(outcome);
                    }
                }
            }
        }
    }

    async fn reload(
        &mut self,
        Reload {
            monitor_id,
            slot,
            slots,
        }: Reload,
    ) {
        if let Some(task) = self.tasks.remove(&monitor_id) {
            task.handle.abort();
            // Waits for the task to stop, so it cannot record a check after the new one reads
            // the state.
            let _ = task.handle.await;
        }
        match Monitor::find(&self.pool, monitor_id).await {
            Ok(Some(monitor)) if !monitor.paused => {
                let delay = startup_spread(&monitor) * slot / slots.max(1);
                self.spawn(monitor, delay);
            }
            Ok(_) => {}
            Err(e) => tracing::error!(monitor_id, error = %e, "cannot reload monitor"),
        }
    }

    fn spawn(&mut self, monitor: Monitor, first_delay: Duration) {
        let id = monitor.id;
        let scheduler = self.scheduler.clone();
        let task = if monitor.is_push() {
            let (sender, receiver) = mpsc::channel(MONITOR_PING_QUEUE);
            Task {
                handle: tokio::spawn(scheduler.run_push(monitor, receiver)),
                pings: Some(sender),
            }
        } else {
            Task {
                handle: tokio::spawn(scheduler.run(monitor, first_delay)),
                pings: None,
            }
        };
        self.tasks.insert(id, task);
    }
}

fn startup_spread(monitor: &Monitor) -> Duration {
    Duration::from_secs(monitor.interval_s.unsigned_abs()).min(MAX_STARTUP_SPREAD)
}

fn random_below(limit: Duration) -> Duration {
    let millis = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX).max(1);
    Duration::from_millis(getrandom::u64().unwrap_or(0) % millis)
}
