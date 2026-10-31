//! Alerts: turns engine events into messages sent to the monitor's notification channels.
//!
//! The engine pushes notifications into a bounded queue. One dispatcher task gathers the downs
//! and ups happening at the same time, reads the channels of each monitor and hands each channel
//! its messages; channels send in parallel, retrying failures, a few requests at a time. Each
//! delivery's final result goes to `notification_log`, one row per monitor.

pub mod channel;
pub mod message;

use crate::engine::guard::Guard;
use crate::models::{Channel, Delivery, NotificationLog, Settings};
use crate::notify::channel::ChannelConfig;
use crate::notify::message::Message;
use crate::time::now_ms;
use jiff::tz::TimeZone;
use reqwest::{Client, StatusCode};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};
use tokio::time::Instant;

/// Waits before each attempt: the first one is immediate.
pub const RETRY_DELAYS: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_secs(10),
    Duration::from_secs(60),
];
/// Notifications waiting for the dispatcher; more are dropped (and logged).
const QUEUE_SIZE: usize = 1000;
/// How long a `down` or `up` waits for others to send with it.
pub const GROUP_WINDOW: Duration = Duration::from_secs(15);
/// Requests in flight at the same time, across all channels.
const MAX_CONCURRENT_SENDS: usize = 5;
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const ERROR_MAX_CHARS: usize = 300;
const USER_AGENT: &str = concat!("uptime-monitoring/", env!("CARGO_PKG_VERSION"));

/// The monitor an alert is about, as it was when the event happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorRef {
    pub id: i64,
    pub name: String,
    /// `None` for a push monitor, which has no target.
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The failure threshold was reached; the outage started at `since`.
    Down { since: i64, cause: String },
    /// The outage that started at `down_since` is over.
    Up { down_since: i64 },
    /// The certificate expires within the monitor's warning period, or has expired.
    TlsExpiring { expires_at: i64 },
    /// Responses stayed above the latency threshold; the slowdown started at `since`.
    Degraded {
        since: i64,
        latency_ms: i64,
        threshold_ms: i64,
    },
    /// The slowdown that started at `degraded_since` is over.
    LatencyNormal { degraded_since: i64 },
    /// Sent from the alerts page to try a channel.
    Test,
}

impl Event {
    /// As stored in `notification_log.event`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Down { .. } => "down",
            Self::Up { .. } => "up",
            Self::TlsExpiring { .. } => "tls_expiring",
            Self::Degraded { .. } => "degraded",
            Self::LatencyNormal { .. } => "latency_normal",
            Self::Test => "test",
        }
    }

    /// Sent in one message with the same event of other monitors happening at the same time.
    fn groups(&self) -> bool {
        matches!(self, Self::Down { .. } | Self::Up { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// `None` for a test.
    pub monitor: Option<MonitorRef>,
    pub event: Event,
    pub at: i64,
}

/// Hands notifications to the dispatcher. The default notifier has no dispatcher and drops
/// them, which is what tests of the web layer use; it can still send tests.
#[derive(Clone, Default)]
pub struct Notifier {
    queue: Option<mpsc::Sender<Notification>>,
    sender: Arc<Sender>,
}

impl Notifier {
    /// Starts the dispatcher.
    pub fn start(pool: SqlitePool, guard: Guard, public_url: Option<String>) -> Self {
        Self::start_with(pool, guard, public_url, RETRY_DELAYS.to_vec(), GROUP_WINDOW)
    }

    /// Like `start`, with other waits between attempts and another grouping window.
    pub fn start_with(
        pool: SqlitePool,
        guard: Guard,
        public_url: Option<String>,
        retry_delays: Vec<Duration>,
        group_window: Duration,
    ) -> Self {
        let sender = Arc::new(Sender {
            guard,
            public_url,
            retry_delays,
            client: OnceLock::new(),
        });
        let (queue, receiver) = mpsc::channel(QUEUE_SIZE);
        tokio::spawn(dispatch(pool, sender.clone(), receiver, group_window));
        Self {
            queue: Some(queue),
            sender,
        }
    }

    /// Never waits: when the queue is full, the notification is dropped.
    pub fn notify(&self, notification: Notification) {
        let Some(queue) = &self.queue else { return };
        match queue.try_send(notification) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(n)) => {
                tracing::error!(
                    event = n.event.as_str(),
                    "notification queue full, alert dropped"
                )
            }
            Err(mpsc::error::TrySendError::Closed(n)) => {
                tracing::error!(
                    event = n.event.as_str(),
                    "notification dispatcher stopped, alert dropped"
                )
            }
        }
    }

    /// Sends a test message once, to a channel that may not be saved yet. Not logged: the
    /// form shows the result.
    pub async fn send_test(&self, config: &ChannelConfig) -> Result<(), String> {
        let notification = Notification {
            monitor: None,
            event: Event::Test,
            at: now_ms(),
        };
        let message = Message::new(&notification, &TimeZone::UTC, None);
        self.sender
            .attempt(config, &notification, &message)
            .await
            .map_err(|e| e.message)
    }
}

/// Sends requests; shared by the dispatcher and the channel form.
struct Sender {
    guard: Guard,
    public_url: Option<String>,
    retry_delays: Vec<Duration>,
    /// Built on first use.
    client: OnceLock<Client>,
}

impl Default for Sender {
    fn default() -> Self {
        Self {
            guard: Guard::default(),
            public_url: None,
            retry_delays: RETRY_DELAYS.to_vec(),
            client: OnceLock::new(),
        }
    }
}

struct SendError {
    message: String,
    /// Worth trying again: no response, a server error, or a rate limit.
    retry: bool,
}

impl Sender {
    fn client(&self) -> &Client {
        self.client.get_or_init(|| {
            // Fails only when a provider is already installed, which is fine.
            let _ = rustls::crypto::ring::default_provider().install_default();
            Client::builder()
                .user_agent(USER_AGENT)
                .timeout(SEND_TIMEOUT)
                .no_proxy()
                .pool_max_idle_per_host(0)
                .redirect(reqwest::redirect::Policy::none())
                .dns_resolver(self.guard.clone())
                .build()
                .expect("static HTTP client configuration")
        })
    }

    /// Tries each delay in turn; returns the last error.
    async fn deliver(
        &self,
        permits: &Semaphore,
        channel: &Channel,
        notification: &Notification,
        message: &Message,
    ) -> Result<(), String> {
        let config = channel
            .parsed_config()
            .map_err(|e| format!("invalid channel configuration: {e}"))?;
        let mut last = String::new();
        for delay in &self.retry_delays {
            tokio::time::sleep(*delay).await;
            let result = {
                let _permit = permits.acquire().await.expect("semaphore never closed");
                self.attempt(&config, notification, message).await
            };
            match result {
                Ok(()) => return Ok(()),
                Err(e) if e.retry => last = e.message,
                Err(e) => return Err(e.message),
            }
        }
        Err(last)
    }

    async fn attempt(
        &self,
        config: &ChannelConfig,
        notification: &Notification,
        message: &Message,
    ) -> Result<(), SendError> {
        // The guard checks the URL actually requested; a URL that does not parse fails here.
        let request = config
            .request(self.client(), notification, message)
            .build()
            .map_err(|_| SendError {
                message: "invalid URL".to_string(),
                retry: false,
            })?;
        self.guard.check_url(request.url()).map_err(|e| SendError {
            message: e.to_string(),
            retry: false,
        })?;
        let response = self
            .client()
            .execute(request)
            .await
            .map_err(|e| SendError {
                // Webhook and Telegram URLs carry their secret.
                message: truncate(&error_chain(&e.without_url())),
                retry: true,
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let retry = status.is_server_error()
            || matches!(
                status,
                StatusCode::TOO_MANY_REQUESTS | StatusCode::REQUEST_TIMEOUT
            );
        let mut message = format!("HTTP {}", status.as_u16());
        if matches!(config, ChannelConfig::Telegram(_))
            && let Some(description) = telegram_description(response).await
        {
            message = truncate(&format!("{message}: {description}"));
        }
        Err(SendError { message, retry })
    }
}

/// The reason Telegram gives for a refusal ("Bad Request: chat not found"), read from at most
/// 4 KB of its answer.
async fn telegram_description(mut response: reqwest::Response) -> Option<String> {
    const MAX_BYTES: usize = 4096;
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BYTES {
            return None;
        }
    }
    let json: serde_json::Value = serde_json::from_slice(&body).ok()?;
    json["description"].as_str().map(str::to_string)
}

/// Sends notifications as they come, except that a `down` or `up` opens a window of
/// `group_window`: everything received until it closes is sent together, so that each channel
/// gets one message for several monitors down (or up) at once.
async fn dispatch(
    pool: SqlitePool,
    sender: Arc<Sender>,
    mut receiver: mpsc::Receiver<Notification>,
    group_window: Duration,
) {
    let mut router = Router {
        pool,
        sender,
        permits: Arc::new(Semaphore::new(MAX_CONCURRENT_SENDS)),
        workers: HashMap::new(),
    };
    let mut batch = Vec::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let flush_at = deadline;
        tokio::select! {
            received = receiver.recv() => {
                let Some(notification) = received else { break };
                if notification.monitor.is_none() {
                    continue;
                }
                if deadline.is_none() {
                    if !notification.event.groups() {
                        router.route(vec![notification]).await;
                        continue;
                    }
                    deadline = Some(Instant::now() + group_window);
                }
                batch.push(notification);
            }
            _ = tokio::time::sleep_until(flush_at.unwrap_or_else(Instant::now)), if flush_at.is_some() => {
                deadline = None;
                router.route(std::mem::take(&mut batch)).await;
            }
        }
    }
    router.route(batch).await;
}

/// One message to send to a channel, about one notification or a group of them.
struct Job {
    channel: Channel,
    notifications: Vec<Arc<Notification>>,
    message: Message,
}

/// Turns notifications into jobs for each channel's worker, started on the channel's first job.
struct Router {
    pool: SqlitePool,
    sender: Arc<Sender>,
    permits: Arc<Semaphore>,
    workers: HashMap<i64, mpsc::UnboundedSender<Job>>,
}

impl Router {
    async fn route(&mut self, notifications: Vec<Notification>) {
        if notifications.is_empty() {
            return;
        }
        // Channels in the order they first appear, each with its notifications in order.
        let mut per_channel: Vec<(Channel, Vec<Arc<Notification>>)> = Vec::new();
        for notification in notifications {
            let monitor_id = notification.monitor.as_ref().map_or(0, |m| m.id);
            let channels = match Channel::enabled_for_monitor(&self.pool, monitor_id).await {
                Ok(channels) => channels,
                Err(e) => {
                    tracing::error!(monitor_id, error = %e, "cannot read notification channels");
                    continue;
                }
            };
            let notification = Arc::new(notification);
            for channel in channels {
                match per_channel.iter_mut().find(|(c, _)| c.id == channel.id) {
                    Some((_, list)) => list.push(notification.clone()),
                    None => per_channel.push((channel, vec![notification.clone()])),
                }
            }
        }
        if per_channel.is_empty() {
            return;
        }
        let tz = match Settings::load(&self.pool).await {
            Ok(settings) => settings.tz(),
            Err(_) => TimeZone::UTC,
        };
        let sender = self.sender.clone();
        let public_url = sender.public_url.as_deref();
        for (channel, list) in per_channel {
            // Webhooks are read by programs, which expect one event per monitor.
            let groups = if channel.kind == "webhook" {
                (0..list.len()).map(|i| vec![i]).collect()
            } else {
                group(&list)
            };
            for indexes in groups {
                let notifications: Vec<_> = indexes.iter().map(|&i| list[i].clone()).collect();
                let message = match notifications.as_slice() {
                    [one] => Message::new(one, &tz, public_url),
                    many => {
                        Message::group(&many.iter().map(|n| &**n).collect::<Vec<_>>(), public_url)
                    }
                };
                let job = Job {
                    channel: channel.clone(),
                    notifications,
                    message,
                };
                // The worker never stops while its sender lives.
                let _ = self.worker(channel.id).send(job);
            }
        }
    }

    fn worker(&mut self, channel_id: i64) -> &mpsc::UnboundedSender<Job> {
        self.workers.entry(channel_id).or_insert_with(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(channel_worker(
                self.pool.clone(),
                self.sender.clone(),
                self.permits.clone(),
                rx,
            ));
            tx
        })
    }
}

/// Splits a channel's notifications into messages, in sending order: the downs together, the
/// ups together, anything else alone. A notification never joins a message sent before one
/// already holding its monitor, so each monitor's alerts keep their order.
fn group(notifications: &[Arc<Notification>]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, notification) in notifications.iter().enumerate() {
        let monitor = |j: &usize| notifications[*j].monitor.as_ref().map(|m| m.id);
        let id = notification.monitor.as_ref().map(|m| m.id);
        let after = groups
            .iter()
            .rposition(|g| g.iter().any(|j| monitor(j) == id))
            .map_or(0, |p| p + 1);
        let kind = notification.event.as_str();
        let joins = notification.event.groups().then(|| {
            groups[after..]
                .iter()
                .position(|g| notifications[g[0]].event.as_str() == kind)
        });
        match joins.flatten() {
            Some(p) => groups[after + p].push(i),
            None => groups.push(vec![i]),
        }
    }
    groups
}

/// Sends one channel's messages one after the other, so that a retried `down` never arrives
/// after the `up` that followed it. Channels are served in parallel.
async fn channel_worker(
    pool: SqlitePool,
    sender: Arc<Sender>,
    permits: Arc<Semaphore>,
    mut receiver: mpsc::UnboundedReceiver<Job>,
) {
    while let Some(job) = receiver.recv().await {
        let result = sender
            .deliver(&permits, &job.channel, &job.notifications[0], &job.message)
            .await;
        if let Err(e) = &result {
            tracing::warn!(channel = %job.channel.name, error = %e, "alert not delivered");
        }
        for notification in &job.notifications {
            if let Err(e) =
                log_delivery(&pool, &job.channel, notification, result.clone().err()).await
            {
                tracing::error!(error = %e, "cannot log the delivery");
            }
        }
    }
}

async fn log_delivery(
    pool: &SqlitePool,
    channel: &Channel,
    notification: &Notification,
    error: Option<String>,
) -> Result<(), sqlx::Error> {
    NotificationLog::insert(
        pool,
        &Delivery {
            channel_id: channel.id,
            monitor_id: notification.monitor.as_ref().map(|m| m.id),
            event: notification.event.as_str(),
            sent_at: now_ms(),
            error: error.as_deref(),
        },
    )
    .await
}

/// `error sending request: … : connection refused`: reqwest's own text hides the cause.
fn error_chain(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(e) = source {
        text.push_str(": ");
        text.push_str(&e.to_string());
        source = e.source();
    }
    text
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= ERROR_MAX_CHARS {
        return text.to_string();
    }
    text.chars()
        .take(ERROR_MAX_CHARS - 1)
        .chain(['…'])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(monitor: i64, event: Event) -> Arc<Notification> {
        Arc::new(Notification {
            monitor: Some(MonitorRef {
                id: monitor,
                name: String::new(),
                url: None,
            }),
            event,
            at: 0,
        })
    }

    fn down(monitor: i64) -> Arc<Notification> {
        of(
            monitor,
            Event::Down {
                since: 0,
                cause: String::new(),
            },
        )
    }

    fn up(monitor: i64) -> Arc<Notification> {
        of(monitor, Event::Up { down_since: 0 })
    }

    #[test]
    fn downs_and_ups_are_grouped_by_event() {
        let list = [down(1), down(2), up(3), down(4), up(5)];
        assert_eq!(group(&list), [vec![0, 1, 3], vec![2, 4]]);
    }

    #[test]
    fn other_events_stay_alone() {
        let tls = || of(1, Event::TlsExpiring { expires_at: 0 });
        let list = [tls(), down(2), tls()];
        assert_eq!(group(&list), [vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn a_monitor_keeps_its_order() {
        // 1 comes back up after 2 did: its up cannot join the earlier one.
        let list = [up(2), down(1), up(1), down(3)];
        assert_eq!(group(&list), [vec![0], vec![1, 3], vec![2]]);
    }
}
