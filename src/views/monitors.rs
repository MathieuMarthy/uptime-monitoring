use crate::error::AppError;
use crate::forms::monitor::{FieldErrors, METHODS, MonitorForm};
use crate::models::{
    Channel, Check, Incident, Monitor, MonitorState, MonitorStatus, MonitorSummary, Settings,
};
use crate::time::{day_of, days_left, format_datetime, format_duration, format_time, now_ms};
use crate::views::heartbeat::{Axis, DailyHistory, Heartbeat};
use crate::views::incidents::{Context, Following, IncidentDetail, error_kind_label};
use askama::Template;
use askama_web::WebTemplate;
use jiff::tz::TimeZone;
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

    pub fn status_key(&self) -> &'static str {
        status_key(self.monitor.paused, self.state.status)
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

/// `up`, `down`, `pending` or `paused`: the suffix of the `lamp-` and `row-` classes.
pub fn status_key(paused: bool, status: MonitorStatus) -> &'static str {
    if paused {
        return "paused";
    }
    match status {
        MonitorStatus::Pending => "pending",
        MonitorStatus::Up => "up",
        MonitorStatus::Down => "down",
    }
}

/// Dashboard order: down, pending, up, then paused.
fn status_rank(paused: bool, status: MonitorStatus) -> u8 {
    match (paused, status) {
        (true, _) => 3,
        (false, MonitorStatus::Down) => 0,
        (false, MonitorStatus::Pending) => 1,
        (false, MonitorStatus::Up) => 2,
    }
}

/// A dashboard row: the monitor and its bars over the chosen range.
pub struct MonitorRow {
    pub monitor: MonitorSummary,
    pub heartbeat: Heartbeat,
}

impl MonitorRow {
    pub fn status_label(&self) -> &'static str {
        status_label(self.monitor.paused, self.monitor.status)
    }

    pub fn status_key(&self) -> &'static str {
        status_key(self.monitor.paused, self.monitor.status)
    }
}

/// The dashboard's time range: 24 hourly bars or 90 daily ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Hours,
    Days,
}

impl Range {
    /// `90d` for the daily bars; anything else shows the hourly ones.
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("90d") => Self::Days,
            _ => Self::Hours,
        }
    }
}

/// A monitor named in the headline, followed by the separator that comes after it.
pub struct NamedMonitor {
    pub id: i64,
    pub name: String,
    pub separator: &'static str,
}

/// The sentence on top of the dashboard: the state that matters most, then the other counts.
pub struct Headline {
    /// Suffix of the lamp class.
    pub state: &'static str,
    pub text: String,
    /// The down monitors, linked under the headline.
    pub down: Vec<NamedMonitor>,
    /// `1 pending, 8 up, 1 paused.`, without the count the headline already gives.
    pub others: String,
}

impl Headline {
    /// `down` lists the down monitors' ids and names, in dashboard order.
    pub fn new(up: usize, pending: usize, paused: usize, down: Vec<(i64, String)>) -> Self {
        let (state, text) = match () {
            _ if !down.is_empty() => (
                "down",
                match down.len() {
                    1 => "1 monitor is down".to_string(),
                    n => format!("{n} monitors are down"),
                },
            ),
            _ if pending > 0 => ("pending", "No monitor is down".to_string()),
            _ if up > 0 => ("up", "All monitors are up".to_string()),
            _ if paused > 0 => ("paused", "Every monitor is paused".to_string()),
            _ => ("paused", "No monitors yet".to_string()),
        };
        let others: Vec<String> = [(pending, "pending"), (up, "up"), (paused, "paused")]
            .into_iter()
            .filter(|&(count, _)| count > 0)
            .map(|(count, label)| format!("{count} {label}"))
            .collect();
        let others = if others.is_empty() {
            if down.is_empty() {
                "Add a monitor to start checking a site.".to_string()
            } else {
                String::new()
            }
        } else {
            format!("{}.", others.join(", "))
        };
        let count = down.len();
        let down = down
            .into_iter()
            .enumerate()
            .map(|(i, (id, name))| NamedMonitor {
                id,
                name,
                separator: match count - i {
                    1 => ".",
                    2 => " and ",
                    _ => ", ",
                },
            })
            .collect();
        Self {
            state,
            text,
            down,
            others,
        }
    }
}

fn missing_state(monitor_id: i64) -> AppError {
    AppError::Internal(format!("monitor {monitor_id} has no monitor_state row").into())
}

#[derive(Template, WebTemplate)]
#[template(path = "monitors/index.html")]
pub struct IndexTemplate {
    pub headline: Headline,
    pub rows: Vec<MonitorRow>,
    pub range: Range,
    pub axis: Axis,
    pub csrf_token: String,
}

impl IndexTemplate {
    /// Bars per row: 24 or 90.
    pub fn bars(&self) -> usize {
        self.axis.labels.len()
    }

    pub fn range_label(&self) -> &'static str {
        match self.range {
            Range::Hours => "Last 24 hours",
            Range::Days => "Last 90 days",
        }
    }

    pub async fn load(
        pool: &SqlitePool,
        range: Range,
        csrf_token: String,
    ) -> Result<Self, AppError> {
        let tz = Settings::load(pool).await?.tz();
        let now = now_ms();
        let mut monitors = Monitor::summaries(pool).await?;
        monitors.sort_by_key(|m| status_rank(m.paused, m.status));
        let (mut heartbeats, axis) = match range {
            Range::Hours => (
                Heartbeat::hourly(pool, None, &tz, now).await?,
                Axis::hours(&tz, now),
            ),
            Range::Days => {
                let ids: Vec<i64> = monitors.iter().map(|m| m.id).collect();
                let daily = DailyHistory::load_many(pool, &ids, &tz, now).await?;
                let bars = daily.into_iter().map(|(id, d)| (id, d.bars)).collect();
                (bars, Axis::days(&tz, now))
            }
        };
        let rows: Vec<MonitorRow> = monitors
            .into_iter()
            .map(|monitor| MonitorRow {
                heartbeat: heartbeats
                    .remove(&monitor.id)
                    .unwrap_or_else(|| Heartbeat::empty(&tz, now)),
                monitor,
            })
            .collect();

        let active_with = |status| {
            rows.iter()
                .filter(|r| !r.monitor.paused && r.monitor.status == status)
                .count()
        };
        let down = rows
            .iter()
            .filter(|r| !r.monitor.paused && r.monitor.status == MonitorStatus::Down)
            .map(|r| (r.monitor.id, r.monitor.name.clone()))
            .collect();
        let headline = Headline::new(
            active_with(MonitorStatus::Up),
            active_with(MonitorStatus::Pending),
            rows.iter().filter(|r| r.monitor.paused).count(),
            down,
        );

        Ok(Self {
            headline,
            rows,
            range,
            axis,
            csrf_token,
        })
    }
}

/// Incidents and checks listed on a monitor's page.
const RECENT: i64 = 20;
/// Heights a latency bar can take, the slowest hour reaching the last.
const LATENCY_LEVELS: i64 = 20;

#[derive(Template, WebTemplate)]
#[template(path = "monitors/detail.html")]
pub struct DetailTemplate {
    pub view: MonitorView,
    /// What the monitor is doing, in a sentence.
    pub status_line: String,
    pub hourly: Heartbeat,
    pub hour_axis: Axis,
    pub latency: Vec<LatencyBar>,
    pub latency_max_ms: Option<i64>,
    /// The hour of the slowest average, for screen readers.
    pub latency_slowest: String,
    pub daily: DailyHistory,
    pub day_axis: Axis,
    pub certificate: String,
    pub incidents: Vec<IncidentDetail>,
    pub checks: Vec<CheckRow>,
    pub csrf_token: String,
}

/// One hour of the latency chart. The CSP forbids inline styles, so its height is one of
/// `LATENCY_LEVELS` classes (`lat-1` to `lat-20`); 0 without a response that hour.
pub struct LatencyBar {
    pub level: i64,
    /// `latency-down` for an hour with downtime.
    pub class: &'static str,
    /// The tooltip; empty without a response.
    pub title: String,
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
        let (latency, latency_max_ms) = latency_bars(&hourly);
        let latency_slowest = hourly
            .ticks
            .iter()
            .filter(|t| t.latency_ms.is_some() && t.latency_ms == latency_max_ms)
            .map(|t| t.span.clone())
            .next_back()
            .unwrap_or_default();

        let certificate = match view.state.tls_expires_at.map(|at| days_left(at, now)) {
            None => "—".to_string(),
            Some(left) if left < 0 => "Expired".to_string(),
            Some(1) => "1 day left".to_string(),
            Some(left) => format!("{left} days left"),
        };
        let recent = Incident::recent(pool, id, RECENT).await?;
        let status_line = status_line(&view, recent.first(), &tz, now);
        let mut incidents = Vec::with_capacity(recent.len());
        let mut following = None;
        for incident in recent {
            // Latest first: the incident loaded just before is the one that followed.
            let next = Following {
                started_at: incident.started_at,
                resolved_at: incident.resolved_at,
            };
            incidents.push(
                IncidentDetail::load(
                    pool,
                    &tz,
                    incident,
                    &view.monitor.name,
                    following.replace(next),
                    Context::Monitor,
                    now,
                )
                .await?,
            );
        }
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
            status_line,
            hourly,
            hour_axis: Axis::hours(&tz, now),
            latency,
            latency_max_ms,
            latency_slowest,
            daily,
            day_axis: Axis::days(&tz, now),
            certificate,
            incidents,
            checks,
            csrf_token,
        })
    }
}

/// The hourly average latencies, scaled to the tallest one.
fn latency_bars(hourly: &Heartbeat) -> (Vec<LatencyBar>, Option<i64>) {
    let max = hourly.ticks.iter().filter_map(|t| t.latency_ms).max();
    let Some(max) = max.filter(|&m| m > 0) else {
        return (Vec::new(), None);
    };
    let bars = hourly
        .ticks
        .iter()
        .map(|tick| LatencyBar {
            level: tick.latency_ms.map_or(0, |ms| {
                ((ms * LATENCY_LEVELS + max - 1) / max).clamp(1, LATENCY_LEVELS)
            }),
            class: if matches!(tick.class, "tick-down" | "tick-dip") {
                "latency-down"
            } else {
                "latency"
            },
            title: tick.latency_ms.map_or_else(String::new, |ms| {
                format!("{}: {ms} ms on average", tick.span)
            }),
        })
        .collect();
    (bars, Some(max))
}

/// `Down for 14 min. HTTP 502 since 14:21.`, `Up since 2026-10-28 09:41.`…
fn status_line(view: &MonitorView, latest: Option<&Incident>, tz: &TimeZone, now: i64) -> String {
    let state = &view.state;
    if view.monitor.paused {
        return "Paused: no checks until you resume it.".to_string();
    }
    match state.status {
        MonitorStatus::Down => match latest.filter(|i| i.resolved_at.is_none()) {
            Some(i) => format!(
                "Down for {}. {} since {}.",
                format_duration(now - i.started_at),
                i.cause
                    .as_deref()
                    .unwrap_or_else(|| error_kind_label(Some(&i.error_kind))),
                if day_of(tz, i.started_at) == day_of(tz, now) {
                    format_time(tz, i.started_at)
                } else {
                    format_datetime(tz, i.started_at)
                }
            ),
            None => "Down.".to_string(),
        },
        MonitorStatus::Pending if state.last_check_at.is_none() => {
            "Waiting for the first check.".to_string()
        }
        MonitorStatus::Pending if state.consecutive_failures > 0 => format!(
            "Retrying: {} of {} failures before down. {}",
            state.consecutive_failures,
            view.monitor.failure_threshold,
            state.last_error.as_deref().unwrap_or_default()
        )
        .trim_end()
        .to_string(),
        MonitorStatus::Pending => "Pending.".to_string(),
        MonitorStatus::Up => match latest.and_then(|i| i.resolved_at) {
            Some(at) => format!("Up since {}.", format_datetime(tz, at)),
            None => "Up, no outage recorded.".to_string(),
        },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::heartbeat::Tick;

    fn tick(class: &'static str, span: &str, latency_ms: Option<i64>) -> Tick {
        Tick {
            class,
            title: String::new(),
            span: span.to_string(),
            checks: 1,
            failed: 0,
            uptime: String::new(),
            latency_ms,
        }
    }

    #[test]
    fn latency_bars_scale_to_the_slowest_hour() {
        let hourly = Heartbeat {
            ticks: vec![
                tick("tick-up", "00:00–01:00", Some(200)),
                tick("tick-none", "01:00–02:00", None),
                tick("tick-down", "02:00–03:00", Some(100)),
                tick("tick-up", "03:00–04:00", Some(1)),
            ],
            monitored_ms: 0,
            downtime_ms: 0,
            latency_avg_ms: None,
        };
        let (bars, max) = latency_bars(&hourly);
        assert_eq!(max, Some(200));
        let levels: Vec<_> = bars.iter().map(|b| b.level).collect();
        assert_eq!(
            levels,
            [20, 0, 10, 1],
            "one bar per hour, empty without a response"
        );
        assert_eq!(bars[2].class, "latency-down");
        assert_eq!(bars[0].title, "00:00–01:00: 200 ms on average");
        assert_eq!(bars[1].title, "");
    }

    fn named(headline: &Headline) -> String {
        headline
            .down
            .iter()
            .map(|m| format!("{}{}", m.name, m.separator))
            .collect()
    }

    #[test]
    fn headline_puts_down_monitors_first() {
        let h = Headline::new(
            8,
            1,
            0,
            vec![(1, "A".into()), (2, "B".into()), (3, "C".into())],
        );
        assert_eq!((h.state, h.text.as_str()), ("down", "3 monitors are down"));
        assert_eq!(named(&h), "A, B and C.");
        assert_eq!(h.others, "1 pending, 8 up.");

        let h = Headline::new(0, 0, 0, vec![(1, "A".into())]);
        assert_eq!(h.text, "1 monitor is down");
        assert_eq!((named(&h).as_str(), h.others.as_str()), ("A.", ""));
    }

    #[test]
    fn headline_without_down_monitors() {
        let h = Headline::new(3, 1, 0, vec![]);
        assert_eq!(
            (h.state, h.text.as_str()),
            ("pending", "No monitor is down")
        );
        let h = Headline::new(3, 0, 2, vec![]);
        assert_eq!((h.state, h.text.as_str()), ("up", "All monitors are up"));
        assert_eq!(h.others, "3 up, 2 paused.");
        let h = Headline::new(0, 0, 2, vec![]);
        assert_eq!(h.text, "Every monitor is paused");
        let h = Headline::new(0, 0, 0, vec![]);
        assert_eq!(h.text, "No monitors yet");
        assert_eq!(h.others, "Add a monitor to start checking a site.");
    }
}
