use crate::error::AppError;
use crate::forms::channel::ChannelForm;
use crate::forms::monitor::FieldErrors;
use crate::models::{Channel, NotificationLog, Settings};
use crate::notify::channel::KINDS;
use crate::time::format_datetime;
use askama::Template;
use askama_web::WebTemplate;
use sqlx::SqlitePool;

/// Deliveries listed on the alerts page.
const RECENT: i64 = 50;

#[derive(Template, WebTemplate)]
#[template(path = "alerts/index.html")]
pub struct IndexTemplate {
    pub channels: Vec<ChannelRow>,
    pub deliveries: Vec<DeliveryRow>,
    pub csrf_token: String,
}

pub struct ChannelRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub monitors: i64,
    /// `None` before the first delivery.
    pub last: Option<(String, bool)>,
}

pub struct DeliveryRow {
    pub time: String,
    pub channel: String,
    pub monitor: String,
    pub event: &'static str,
    pub ok: bool,
    pub error: String,
}

impl IndexTemplate {
    pub async fn load(pool: &SqlitePool, csrf_token: String) -> Result<Self, AppError> {
        let tz = Settings::load(pool).await?.tz();
        let channels = Channel::summaries(pool)
            .await?
            .into_iter()
            .map(|c| ChannelRow {
                id: c.id,
                name: c.name,
                kind: c.kind,
                enabled: c.enabled,
                monitors: c.monitors,
                last: c
                    .last_sent_at
                    .zip(c.last_ok)
                    .map(|(at, ok)| (format_datetime(&tz, at), ok)),
            })
            .collect();
        let deliveries = NotificationLog::recent(pool, RECENT)
            .await?
            .into_iter()
            .map(|d| DeliveryRow {
                time: format_datetime(&tz, d.sent_at),
                channel: d.channel_name.unwrap_or_else(|| "(deleted)".to_string()),
                monitor: match (d.event.as_str(), d.monitor_name) {
                    ("test", _) => "—".to_string(),
                    (_, Some(name)) => name,
                    (_, None) => "(deleted)".to_string(),
                },
                event: event_label(&d.event),
                ok: d.ok,
                error: d.error.unwrap_or_default(),
            })
            .collect();
        Ok(Self {
            channels,
            deliveries,
            csrf_token,
        })
    }
}

fn event_label(event: &str) -> &'static str {
    match event {
        "down" => "Down",
        "up" => "Up",
        "tls_expiring" => "Certificate",
        _ => "Test",
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "alerts/form.html")]
pub struct FormTemplate {
    pub form: ChannelForm,
    pub errors: FieldErrors,
    pub csrf_token: String,
    pub action_url: String,
    pub channel_id: Option<i64>,
    /// Editing an ntfy channel that has a token.
    pub has_token: bool,
}

impl FormTemplate {
    pub fn is_edit(&self) -> bool {
        self.channel_id.is_some()
    }

    pub fn error(&self, field: &str) -> Option<&str> {
        self.errors.get(field).map(String::as_str)
    }

    pub fn kinds(&self) -> &'static [&'static str] {
        &KINDS
    }
}
