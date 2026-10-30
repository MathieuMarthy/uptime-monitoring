//! An incident unfolded: its facts, its failed checks and the alerts sent about it. Shown on a
//! monitor's page and on the Activity page.

use crate::error::AppError;
use crate::forms::incident::NOTE_MAX_CHARS;
use crate::models::{Check, FailedCheck, Incident, LogEntry, NotificationLog};
use crate::time::{format_datetime, format_duration, format_time};
use jiff::tz::TimeZone;
use sqlx::SqlitePool;

/// Failed checks listed under an incident; the others are only counted.
pub const SHOWN_FAILURES: i64 = 10;
/// How long after its transition an alert can be logged: it is logged once delivered, retries
/// included (up to 70 s).
pub const ALERT_DELAY_MS: i64 = 5 * 60 * 1000;

/// Where the incident is shown, which decides how it is titled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// On the monitor's own page: dated in full, titled by its cause.
    Monitor,
    /// In the Activity feed, grouped by day: timed, titled by the monitor.
    Activity,
}

/// The incident that followed on the same monitor, if any: the alerts sent after it began are
/// its own.
#[derive(Debug, Clone, Copy)]
pub struct Following {
    pub started_at: i64,
    pub resolved_at: Option<i64>,
}

pub struct Fact {
    pub label: &'static str,
    pub value: String,
}

pub struct FailureRow {
    pub time: String,
    pub status_code: Option<i64>,
    pub latency_ms: Option<i64>,
    pub error: String,
}

impl FailureRow {
    pub fn new(tz: &TimeZone, check: &FailedCheck) -> Self {
        Self {
            time: format_datetime_seconds(tz, check.checked_at),
            status_code: check.status_code,
            latency_ms: check.latency_ms,
            error: check
                .error
                .clone()
                .unwrap_or_else(|| error_kind_label(check.error_kind.as_deref()).to_string()),
        }
    }
}

pub struct AlertRow {
    pub time: String,
    pub channel: String,
    pub event: &'static str,
    pub ok: bool,
    pub error: Option<String>,
}

impl AlertRow {
    pub fn new(tz: &TimeZone, entry: LogEntry) -> Self {
        Self {
            time: format_time(tz, entry.sent_at),
            channel: entry
                .channel_name
                .unwrap_or_else(|| "Deleted channel".to_string()),
            event: match entry.event.as_str() {
                "down" => "down alert",
                "up" => "back up alert",
                "tls_expiring" => "certificate alert",
                _ => "test",
            },
            ok: entry.ok,
            error: entry.error,
        }
    }
}

pub struct IncidentDetail {
    pub id: i64,
    pub monitor_id: i64,
    pub monitor_name: String,
    /// In the Activity feed, a link leads to the monitor.
    pub link_monitor: bool,
    pub ongoing: bool,
    pub time: String,
    pub title: String,
    /// Under the title; empty when the title already is the cause.
    pub subtitle: String,
    /// `Ongoing, 14 min` or the duration.
    pub tag: String,
    pub facts: Vec<Fact>,
    pub failures: Vec<FailureRow>,
    /// Failures in the incident beyond those listed.
    pub more_failures: i64,
    pub alerts: Vec<AlertRow>,
    pub note: Option<String>,
    pub note_public: bool,
}

impl IncidentDetail {
    /// The note's `maxlength`, the server's limit.
    pub fn note_max(&self) -> usize {
        NOTE_MAX_CHARS
    }

    pub async fn load(
        pool: &SqlitePool,
        tz: &TimeZone,
        incident: Incident,
        monitor_name: &str,
        following: Option<Following>,
        context: Context,
        now: i64,
    ) -> Result<Self, AppError> {
        let end = incident.resolved_at.unwrap_or(now);
        let failed = Check::failures_between(
            pool,
            incident.monitor_id,
            incident.started_at,
            end,
            SHOWN_FAILURES,
        )
        .await?;
        let total = failed.first().map_or(0, |f| f.total);
        // Its down alert comes before the next incident starts; its up alert follows its end,
        // before the next incident's own up alert.
        let down_until = following.map_or(end + ALERT_DELAY_MS, |f| f.started_at - 1);
        let up_until = (end + ALERT_DELAY_MS).min(
            following
                .and_then(|f| f.resolved_at)
                .map_or(i64::MAX, |at| at - 1),
        );
        let alerts: Vec<LogEntry> = NotificationLog::for_monitor_between(
            pool,
            incident.monitor_id,
            &["down", "up"],
            incident.started_at,
            down_until.max(up_until),
        )
        .await?
        .into_iter()
        .filter(|a| match a.event.as_str() {
            "down" => a.sent_at <= down_until,
            _ => incident.resolved_at.is_some_and(|at| a.sent_at >= at) && a.sent_at <= up_until,
        })
        .collect();

        let duration = format_duration(end - incident.started_at);
        let cause = incident
            .cause
            .clone()
            .unwrap_or_else(|| error_kind_label(Some(&incident.error_kind)).to_string());
        let ongoing = incident.resolved_at.is_none();
        let (time, title, subtitle) = match context {
            Context::Monitor => (
                format_datetime(tz, incident.started_at),
                cause,
                String::new(),
            ),
            Context::Activity => (
                format_time(tz, incident.started_at),
                if ongoing {
                    format!("{monitor_name} is down")
                } else {
                    format!("{monitor_name} was down {duration}")
                },
                cause,
            ),
        };

        let mut facts = vec![
            Fact {
                label: "Started",
                value: format_datetime_seconds(tz, incident.started_at),
            },
            Fact {
                label: "Resolved",
                value: incident.resolved_at.map_or_else(
                    || "Not yet".to_string(),
                    |at| format_datetime_seconds(tz, at),
                ),
            },
            Fact {
                label: "Duration",
                value: duration.clone(),
            },
            Fact {
                label: "Error type",
                value: error_kind_label(Some(&incident.error_kind)).to_string(),
            },
        ];
        if total > 0 {
            facts.push(Fact {
                label: "Failed checks",
                value: total.to_string(),
            });
        }

        Ok(Self {
            note: incident.note.clone(),
            note_public: incident.note_public,
            id: incident.id,
            monitor_id: incident.monitor_id,
            monitor_name: monitor_name.to_string(),
            link_monitor: context == Context::Activity,
            ongoing,
            time,
            title,
            subtitle,
            tag: if ongoing {
                format!("Ongoing, {duration}")
            } else {
                duration
            },
            facts,
            more_failures: total - failed.len() as i64,
            failures: failed.iter().map(|f| FailureRow::new(tz, f)).collect(),
            alerts: alerts.into_iter().map(|a| AlertRow::new(tz, a)).collect(),
        })
    }
}

/// How a check failed, in words.
pub fn error_kind_label(kind: Option<&str>) -> &'static str {
    match kind {
        Some("timeout") => "Timeout",
        Some("dns") => "DNS lookup",
        Some("blocked") => "Blocked address",
        Some("connect") => "Connection",
        Some("tls") => "TLS certificate",
        Some("status") => "HTTP status",
        Some("keyword") => "Keyword",
        Some("missed") => "Missed ping",
        Some("reported") => "Reported by the job",
        _ => "Unknown",
    }
}

/// `2026-10-28 14:05:07`: failures can be seconds apart.
fn format_datetime_seconds(tz: &TimeZone, ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| {
            t.to_zoned(tz.clone())
                .strftime("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}
