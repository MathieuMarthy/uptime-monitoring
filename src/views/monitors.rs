use crate::error::AppError;
use crate::forms::monitor::{FieldErrors, METHODS, MonitorForm};
use crate::models::{
    Channel, Check, Incident, Monitor, MonitorState, MonitorStatus, MonitorSummary, Settings,
};
use crate::time::{day_of, days_left, format_datetime, format_duration, now_ms};
use crate::views::heartbeat::{DailyHistory, Heartbeat};
use askama::Template;
use askama_web::WebTemplate;
use sqlx::SqlitePool;

/// A monitor's configuration together with its runtime state.
pub struct MonitorView {
    pub monitor: Monitor,
    pub state: MonitorState,
}

impl MonitorView {
    pub async fn load(pool: &SqlitePool, id: i64) -> Result<Self, AppError> {
        let monitor = Monitor::find(pool, id).await?.ok_or(AppError::NotFound)?;
        let state = MonitorState::find(pool, id)
            .await?
            .ok_or_else(|| missing_state(id))?;
        Ok(Self { monitor, state })
    }

    pub fn status_label(&self) -> &'static str {
        status_label(self.monitor.paused, self.state.status)
    }

    pub fn status_class(&self) -> &'static str {
        status_class(self.monitor.paused, self.state.status)
    }
}

pub fn status_label(paused: bool, status: MonitorStatus) -> &'static str {
    if paused {
        return "Paused";
    }
    match status {
        MonitorStatus::Pending => "Pending",
        MonitorStatus::Up => "Up",
        MonitorStatus::Down => "Down",
    }
}

pub fn status_class(paused: bool, status: MonitorStatus) -> &'static str {
    if paused {
        return "text-blue";
    }
    match status {
        MonitorStatus::Pending => "text-muted",
        MonitorStatus::Up => "text-green",
        MonitorStatus::Down => "text-red",
    }
}

/// A dashboard card: the monitor and its last 24 hours.
pub struct MonitorCard {
    pub monitor: MonitorSummary,
    pub heartbeat: Heartbeat,
}

impl MonitorCard {
    pub fn status_label(&self) -> &'static str {
        status_label(self.monitor.paused, self.monitor.status)
    }

    pub fn status_class(&self) -> &'static str {
        status_class(self.monitor.paused, self.monitor.status)
    }
}

fn missing_state(monitor_id: i64) -> AppError {
    AppError::Internal(format!("monitor {monitor_id} has no monitor_state row").into())
}

#[derive(Template, WebTemplate)]
#[template(path = "monitors/index.html")]
pub struct IndexTemplate {
    pub cards: Vec<MonitorCard>,
    pub up: usize,
    pub down: usize,
    pub pending: usize,
    pub paused: usize,
    pub csrf_token: String,
}

impl IndexTemplate {
    pub async fn load(pool: &SqlitePool, csrf_token: String) -> Result<Self, AppError> {
        let tz = Settings::load(pool).await?.tz();
        let now = now_ms();
        let mut heartbeats = Heartbeat::hourly(pool, None, &tz, now).await?;
        let cards: Vec<MonitorCard> = Monitor::summaries(pool)
            .await?
            .into_iter()
            .map(|monitor| MonitorCard {
                heartbeat: heartbeats
                    .remove(&monitor.id)
                    .unwrap_or_else(|| Heartbeat::empty(&tz, now)),
                monitor,
            })
            .collect();

        let active_with = |status| {
            cards
                .iter()
                .filter(|c| !c.monitor.paused && c.monitor.status == status)
                .count()
        };
        let up = active_with(MonitorStatus::Up);
        let down = active_with(MonitorStatus::Down);
        let pending = active_with(MonitorStatus::Pending);
        let paused = cards.iter().filter(|c| c.monitor.paused).count();

        Ok(Self {
            cards,
            up,
            down,
            pending,
            paused,
            csrf_token,
        })
    }
}

/// Incidents and checks listed on a monitor's page.
const RECENT: i64 = 20;

#[derive(Template, WebTemplate)]
#[template(path = "monitors/detail.html")]
pub struct DetailTemplate {
    pub view: MonitorView,
    pub hourly: Heartbeat,
    pub daily: DailyHistory,
    pub tls_expiry: Option<String>,
    pub incidents: Vec<IncidentRow>,
    pub checks: Vec<CheckRow>,
    pub csrf_token: String,
}

pub struct IncidentRow {
    pub started: String,
    /// `None` while the incident is open.
    pub resolved: Option<String>,
    pub duration: String,
    pub cause: String,
}

pub struct CheckRow {
    pub time: String,
    pub ok: bool,
    pub status_code: Option<i64>,
    pub latency_ms: Option<i64>,
    pub error: Option<String>,
}

impl DetailTemplate {
    pub async fn load(pool: &SqlitePool, id: i64, csrf_token: String) -> Result<Self, AppError> {
        let view = MonitorView::load(pool, id).await?;
        let tz = Settings::load(pool).await?.tz();
        let now = now_ms();
        let hourly = Heartbeat::hourly(pool, Some(id), &tz, now)
            .await?
            .remove(&id)
            .unwrap_or_else(|| Heartbeat::empty(&tz, now));
        let daily = DailyHistory::load(pool, id, &tz, now).await?;

        let tls_expiry = view.state.tls_expires_at.map(|at| {
            let date = day_of(&tz, at);
            match days_left(at, now) {
                left if left < 0 => format!("{date} (expired)"),
                1 => format!("{date} (1 day left)"),
                left => format!("{date} ({left} days left)"),
            }
        });
        let incidents = Incident::recent(pool, id, RECENT)
            .await?
            .into_iter()
            .map(|i| IncidentRow {
                started: format_datetime(&tz, i.started_at),
                resolved: i.resolved_at.map(|at| format_datetime(&tz, at)),
                duration: format_duration(i.resolved_at.unwrap_or(now) - i.started_at),
                cause: i.cause.unwrap_or(i.error_kind),
            })
            .collect();
        let checks = Check::recent(pool, id, RECENT)
            .await?
            .into_iter()
            .map(|c| CheckRow {
                time: format_datetime(&tz, c.checked_at),
                ok: c.ok,
                status_code: c.status_code,
                latency_ms: c.latency_ms,
                error: c.error,
            })
            .collect();

        Ok(Self {
            view,
            hourly,
            daily,
            tls_expiry,
            incidents,
            checks,
            csrf_token,
        })
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "monitors/form.html")]
pub struct FormTemplate {
    pub form: MonitorForm,
    pub errors: FieldErrors,
    pub csrf_token: String,
    pub action_url: String,
    pub cancel_url: String,
    pub monitor_id: Option<i64>,
    /// Every channel, sorted by name, for the alert checkboxes.
    pub channels: Vec<Channel>,
}

impl FormTemplate {
    pub fn is_edit(&self) -> bool {
        self.monitor_id.is_some()
    }

    pub fn error(&self, field: &str) -> Option<&str> {
        self.errors.get(field).map(String::as_str)
    }

    pub fn methods(&self) -> &'static [&'static str] {
        &METHODS
    }
}
