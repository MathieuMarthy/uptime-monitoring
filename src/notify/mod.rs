//! Alerts: turns engine events into messages sent to the monitor's notification channels.
//!
//! The engine pushes notifications into a bounded queue. One dispatcher task reads the channels
//! of the monitor and sends to each of them, retrying failures, a few requests at a time. Each
//! delivery's final result goes to `notification_log`.

pub mod channel;
pub mod message;

use crate::engine::guard::Guard;
use crate::models::{Channel, Delivery, NotificationLog, Settings};
use crate::notify::message::Message;
use crate::time::now_ms;
use jiff::tz::TimeZone;
use reqwest::{Client, StatusCode, Url};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

/// Waits before each attempt: the first one is immediate.
pub const RETRY_DELAYS: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_secs(10),
    Duration::from_secs(60),
];
/// Notifications waiting for the dispatcher; more are dropped (and logged).
const QUEUE_SIZE: usize = 1000;
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
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The failure threshold was reached; the outage started at `since`.
    Down { since: i64, cause: String },
    /// The outage that started at `down_since` is over.
    Up { down_since: i64 },
    /// The certificate expires within the monitor's warning period, or has expired.
    TlsExpiring { expires_at: i64 },
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
            Self::Test => "test",
        }
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
    pub fn start(pool: SqlitePool, guard: Guard) -> Self {
        Self::start_with(pool, guard, RETRY_DELAYS.to_vec())
    }

    /// Like `start`, with other waits between attempts.
    pub fn start_with(pool: SqlitePool, guard: Guard, retry_delays: Vec<Duration>) -> Self {
        let sender = Arc::new(Sender {
            guard,
            retry_delays,
            client: OnceLock::new(),
        });
        let (queue, receiver) = mpsc::channel(QUEUE_SIZE);
        tokio::spawn(dispatch(pool, sender.clone(), receiver));
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

    /// Sends a test message to the channel, once, and logs the result.
    pub async fn send_test(&self, pool: &SqlitePool, channel: &Channel) -> Result<(), sqlx::Error> {
        let notification = Notification {
            monitor: None,
            event: Event::Test,
            at: now_ms(),
        };
        let message = Message::new(&notification, &TimeZone::UTC);
        let result = self.sender.attempt(channel, &notification, &message).await;
        log_delivery(
            pool,
            channel,
            &notification,
            result.err().map(|e| e.message),
        )
        .await
    }
}

/// Sends requests; shared by the dispatcher and the alerts page.
struct Sender {
    guard: Guard,
    retry_delays: Vec<Duration>,
    /// Built on first use.
    client: OnceLock<Client>,
}

impl Default for Sender {
    fn default() -> Self {
        Self {
            guard: Guard::default(),
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
        let mut last = String::new();
        for delay in &self.retry_delays {
            tokio::time::sleep(*delay).await;
            let result = {
                let _permit = permits.acquire().await.expect("semaphore never closed");
                self.attempt(channel, notification, message).await
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
        channel: &Channel,
        notification: &Notification,
        message: &Message,
    ) -> Result<(), SendError> {
        let config = channel.parsed_config().map_err(|e| SendError {
            message: format!("invalid channel configuration: {e}"),
            retry: false,
        })?;
        let url = Url::parse(config.target()).map_err(|_| SendError {
            message: "invalid URL".to_string(),
            retry: false,
        })?;
        self.guard.check_url(&url).map_err(|e| SendError {
            message: e.to_string(),
            retry: false,
        })?;
        let response = config
            .request(self.client(), notification, message)
            .send()
            .await
            .map_err(|e| SendError {
                // Webhook URLs carry their secret.
                message: truncate(&error_chain(&e.without_url())),
                retry: true,
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        Err(SendError {
            message: format!("HTTP {}", status.as_u16()),
            retry: status.is_server_error()
                || matches!(
                    status,
                    StatusCode::TOO_MANY_REQUESTS | StatusCode::REQUEST_TIMEOUT
                ),
        })
    }
}

/// Routes each notification to its monitor's worker, started on its first alert.
async fn dispatch(
    pool: SqlitePool,
    sender: Arc<Sender>,
    mut receiver: mpsc::Receiver<Notification>,
) {
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_SENDS));
    let mut workers: HashMap<i64, mpsc::UnboundedSender<Notification>> = HashMap::new();
    while let Some(notification) = receiver.recv().await {
        let Some(monitor_id) = notification.monitor.as_ref().map(|m| m.id) else {
            continue;
        };
        let worker = workers.entry(monitor_id).or_insert_with(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(monitor_worker(
                pool.clone(),
                sender.clone(),
                permits.clone(),
                rx,
            ));
            tx
        });
        // The worker never stops while its sender lives.
        let _ = worker.send(notification);
    }
}

/// Delivers one monitor's notifications one after the other, so that a retried `down` never
/// arrives after the `up` that followed it. The channels of one notification are served in
/// parallel.
async fn monitor_worker(
    pool: SqlitePool,
    sender: Arc<Sender>,
    permits: Arc<Semaphore>,
    mut receiver: mpsc::UnboundedReceiver<Notification>,
) {
    while let Some(notification) = receiver.recv().await {
        let monitor_id = notification.monitor.as_ref().map(|m| m.id);
        let channels = match Channel::enabled_for_monitor(&pool, monitor_id.unwrap_or(0)).await {
            Ok(channels) => channels,
            Err(e) => {
                tracing::error!(monitor_id, error = %e, "cannot read notification channels");
                continue;
            }
        };
        if channels.is_empty() {
            continue;
        }
        let tz = match Settings::load(&pool).await {
            Ok(settings) => settings.tz(),
            Err(_) => TimeZone::UTC,
        };
        let message = Arc::new(Message::new(&notification, &tz));
        let notification = Arc::new(notification);
        let mut deliveries = JoinSet::new();
        for channel in channels {
            let (pool, sender, permits) = (pool.clone(), sender.clone(), permits.clone());
            let (notification, message) = (notification.clone(), message.clone());
            deliveries.spawn(async move {
                let result = sender
                    .deliver(&permits, &channel, &notification, &message)
                    .await;
                if let Err(e) = &result {
                    tracing::warn!(channel = %channel.name, error = %e, "alert not delivered");
                }
                if let Err(e) = log_delivery(&pool, &channel, &notification, result.err()).await {
                    tracing::error!(error = %e, "cannot log the delivery");
                }
            });
        }
        deliveries.join_all().await;
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
