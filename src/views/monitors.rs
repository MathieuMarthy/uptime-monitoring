use crate::engine::verdict::CheckOutcome;
use crate::error::AppError;
use crate::forms::monitor::{FieldErrors, METHODS, MonitorForm, PUSH, UNGROUPED};
use crate::models::{
    Channel, Check, Incident, Monitor, MonitorState, MonitorStatus, MonitorSummary, Settings,
    name_key,
};
use crate::time::{
    DAY_MS, day_of, days_left, format_datetime, format_duration, format_time, now_ms,
};
use crate::views::heartbeat::{Axis, DailyHistory, Heartbeat};
use crate::views::incidents::{Context, Following, IncidentDetail, error_kind_label};
use askama::Template;
use askama_web::WebTemplate;
use jiff::tz::TimeZone;
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

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
    /// What the search matches: name, address and group.
    pub search: String,
    /// Left out by the dashboard filter: rendered hidden, so `filter.js` can show it again.
    pub hidden: bool,
}

impl MonitorRow {
    fn new(monitor: MonitorSummary, heartbeat: Heartbeat) -> Self {
        let group = monitor.group_name.as_deref().unwrap_or("");
        let search = format!("{} {} {group}", monitor.name, monitor.url)
            .trim_end()
            .to_string();
        Self {
            monitor,
            heartbeat,
            search,
            hidden: false,
        }
    }

    pub fn status_label(&self) -> &'static str {
        status_label(self.monitor.paused, self.monitor.status)
    }

    pub fn status_key(&self) -> &'static str {
        status_key(self.monitor.paused, self.monitor.status)
    }
}

/// The monitors of one group on the dashboard, in dashboard order.
pub struct MonitorGroup {
    /// `None` for the monitors without a group.
    pub name: Option<String>,
    /// The `<details>` id, stable across refreshes so `live.js` keeps it open or closed.
    pub id: String,
    /// The worst state of its monitors: the suffix of the lamp class.
    pub state: &'static str,
    /// `1 down, 4 up`.
    pub counts: String,
    pub rows: Vec<MonitorRow>,
    /// Every row hidden by the dashboard filter.
    pub hidden: bool,
}

impl MonitorGroup {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(UNGROUPED)
    }

    /// Groups the rows, given in name order: by group name ignoring case (spelled as on its first
    /// monitor), then ungrouped. Rows keep the dashboard order within a group.
    fn split(rows: Vec<MonitorRow>) -> Vec<Self> {
        let mut groups: BTreeMap<Option<String>, Vec<MonitorRow>> = BTreeMap::new();
        for row in rows {
            groups
                .entry(row.monitor.group_name.as_deref().map(name_key))
                .or_default()
                .push(row);
        }
        // `None` sorts first in the map: move it last.
        let mut groups: Vec<_> = groups.into_iter().collect();
        if groups.first().is_some_and(|(key, _)| key.is_none()) {
            groups.rotate_left(1);
        }
        groups
            .into_iter()
            .map(|(key, mut rows)| {
                let name = rows[0].monitor.group_name.clone();
                rows.sort_by_key(|r| status_rank(r.monitor.paused, r.monitor.status));
                let id = match key {
                    Some(key) => key.bytes().fold("group-".to_string(), |mut id, b| {
                        id.push_str(&format!("{b:02x}"));
                        id
                    }),
                    None => "group-none".to_string(),
                };
                let counts: Vec<String> = ["down", "pending", "up", "paused"]
                    .into_iter()
                    .filter_map(|key| {
                        let count = rows.iter().filter(|r| r.status_key() == key).count();
                        (count > 0).then(|| format!("{count} {key}"))
                    })
                    .collect();
                Self {
                    name,
                    id,
                    state: rows[0].status_key(),
                    counts: counts.join(", "),
                    rows,
                    hidden: false,
                }
            })
            .collect()
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

/// The dashboard's state filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    Down,
    Paused,
}

impl StatusFilter {
    fn parse(value: Option<&str>) -> Option<Self> {
        match value {
            Some("down") => Some(Self::Down),
            Some("paused") => Some(Self::Paused),
            _ => None,
        }
    }

    pub fn param(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Paused => "paused",
        }
    }

    fn matches(self, row: &MonitorRow) -> bool {
        match self {
            Self::Down => row.status_key() == "down",
            Self::Paused => row.monitor.paused,
        }
    }
}

/// Lower case, without accents: what the search compares, as `filter.js` does.
fn fold(text: &str) -> String {
    text.nfd()
        .filter(|&c| !is_combining_mark(c))
        .collect::<String>()
        .to_lowercase()
}

/// What the dashboard shows, read from its URL (`/?range=90d&q=api&status=down&group=Web`) so a
/// filtered list works without JS and can be shared as a link.
pub struct DashboardFilter {
    pub range: Range,
    /// Searched in the name, address and group, ignoring case and accents.
    pub q: String,
    /// `q` folded once for every row.
    folded: String,
    pub status: Option<StatusFilter>,
    /// A group name, ignoring case, or `Ungrouped` for the monitors without one.
    pub group: Option<String>,
}

impl DashboardFilter {
    pub fn parse(
        range: Range,
        q: Option<&str>,
        status: Option<&str>,
        group: Option<&str>,
    ) -> Self {
        let q = q.unwrap_or_default().trim().to_string();
        Self {
            range,
            folded: fold(&q),
            q,
            status: StatusFilter::parse(status),
            group: group
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .map(String::from),
        }
    }

    /// `in_group`: whether the row's group passes the group filter.
    fn matches(&self, row: &MonitorRow, in_group: bool) -> bool {
        in_group
            && self.status.is_none_or(|s| s.matches(row))
            && fold(&row.search).contains(&self.folded)
    }

    /// Whether `label` is the chosen group.
    pub fn is_group(&self, label: &str) -> bool {
        self.group
            .as_deref()
            .is_some_and(|g| name_key(g) == name_key(label))
    }

    /// `/` with this filter, the range and status replaced.
    pub fn url(&self, range: Range, status: Option<StatusFilter>) -> String {
        let mut params = form_urlencoded::Serializer::new(String::new());
        if range == Range::Days {
            params.append_pair("range", "90d");
        }
        if !self.q.is_empty() {
            params.append_pair("q", &self.q);
        }
        if let Some(status) = status {
            params.append_pair("status", status.param());
        }
        if let Some(group) = &self.group {
            params.append_pair("group", group);
        }
        match params.finish() {
            query if query.is_empty() => "/".to_string(),
            query => format!("/?{query}"),
        }
    }

    /// `/` with this filter over another range.
    pub fn range_url(&self, range: Range) -> String {
        self.url(range, self.status)
    }

    /// `/` with only the range: every monitor.
    pub fn reset_url(&self) -> &'static str {
        match self.range {
            Range::Hours => "/",
            Range::Days => "/?range=90d",
        }
    }
}

/// A state filter above the dashboard list, with its count of monitors.
pub struct StatusChip {
    pub label: &'static str,
    /// `data-status` for `filter.js`: empty for every monitor.
    pub key: &'static str,
    pub count: usize,
    pub url: String,
    pub current: bool,
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
    pub groups: Vec<MonitorGroup>,
    /// Whether any monitor has a group: without, the list has no group headers.
    pub grouped: bool,
    pub filter: DashboardFilter,
    pub chips: Vec<StatusChip>,
    /// The chosen group when no monitor is in it.
    pub missing_group: Option<String>,
    /// Whether the filter leaves any row: without, the list says so and offers to reset.
    pub matches: bool,
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
        filter: DashboardFilter,
        csrf_token: String,
    ) -> Result<Self, AppError> {
        let range = filter.range;
        let tz = Settings::load(pool).await?.tz();
        let now = now_ms();
        let monitors = Monitor::summaries(pool).await?;
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
            .map(|monitor| {
                let heartbeat = heartbeats
                    .remove(&monitor.id)
                    .unwrap_or_else(|| Heartbeat::empty(&tz, now));
                MonitorRow::new(monitor, heartbeat)
            })
            .collect();

        let active_with = |status| {
            rows.iter()
                .filter(|r| !r.monitor.paused && r.monitor.status == status)
                .count()
        };
        // Rows are still in name order, the order of the down monitors in the headline.
        let down: Vec<_> = rows
            .iter()
            .filter(|r| !r.monitor.paused && r.monitor.status == MonitorStatus::Down)
            .map(|r| (r.monitor.id, r.monitor.name.clone()))
            .collect();
        let paused = rows.iter().filter(|r| r.monitor.paused).count();
        let chips = [
            ("All", None, rows.len()),
            ("Down", Some(StatusFilter::Down), down.len()),
            ("Paused", Some(StatusFilter::Paused), paused),
        ]
        .into_iter()
        .map(|(label, status, count)| StatusChip {
            label,
            key: status.map_or("", StatusFilter::param),
            count,
            url: filter.url(range, status),
            current: filter.status == status,
        })
        .collect();
        let headline = Headline::new(
            active_with(MonitorStatus::Up),
            active_with(MonitorStatus::Pending),
            paused,
            down,
        );
        let grouped = rows.iter().any(|r| r.monitor.group_name.is_some());
        let mut groups = MonitorGroup::split(rows);
        for group in &mut groups {
            let in_group = filter.group.is_none() || filter.is_group(group.label());
            for row in &mut group.rows {
                row.hidden = !filter.matches(row, in_group);
            }
            group.hidden = group.rows.iter().all(|r| r.hidden);
        }
        let matches = groups.iter().any(|g| !g.hidden);
        // A chosen group no monitor has any more (an old link) stays in the menu, matching nothing.
        let missing_group = filter
            .group
            .clone()
            .filter(|_| groups.iter().all(|g| !filter.is_group(g.label())));

        Ok(Self {
            headline,
            groups,
            grouped,
            filter,
            chips,
            missing_group,
            matches,
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
    /// Push monitors only: the URL their job calls, absolute when `PUBLIC_URL` is set.
    pub push_url: Option<String>,
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
    pub async fn load(
        pool: &SqlitePool,
        id: i64,
        csrf_token: String,
        public_url: Option<&str>,
    ) -> Result<Self, AppError> {
        let view = MonitorView::load(pool, id).await?;
        let push_url = view
            .monitor
            .push_token
            .as_ref()
            .map(|token| format!("{}/push/{token}", public_url.unwrap_or_default()));
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

        let certificate = view
            .state
            .tls_expires_at
            .map_or_else(|| "—".to_string(), |at| certificate_label(at, now));
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
            push_url,
        })
    }
}

/// `Expired`, `1 day left`, `41 days left`.
pub fn certificate_label(expires_at: i64, now: i64) -> String {
    match days_left(expires_at, now) {
        left if left < 0 => "Expired".to_string(),
        1 => "1 day left".to_string(),
        left => format!("{left} days left"),
    }
}

/// What the form's Test button found: `HTTP 200 in 84 ms. Certificate: 41 days left.`, with a
/// note when saving would send a certificate alert at once; or the failure.
pub fn test_summary(outcome: &CheckOutcome, tls_warn_days: Option<i64>) -> Result<String, String> {
    if let Some(failure) = &outcome.failure {
        return Err(failure.message.clone());
    }
    let mut summary = match (outcome.status_code, outcome.latency_ms) {
        (Some(code), Some(ms)) => format!("HTTP {code} in {ms} ms."),
        _ => "The check passed.".to_string(),
    };
    if let Some(expires_at) = outcome.tls_expires_at {
        let label = certificate_label(expires_at, outcome.checked_at);
        // The engine's rule (`MonitorState::record`).
        match tls_warn_days.filter(|days| expires_at < outcome.checked_at + days * DAY_MS) {
            Some(days) => summary.push_str(&format!(
                " Certificate: {label}, within the {days}-day warning: saving sends a certificate alert."
            )),
            None => summary.push_str(&format!(" Certificate: {label}.")),
        }
    }
    Ok(summary)
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
        MonitorStatus::Pending if state.last_check_at.is_none() && view.monitor.is_push() => {
            "Waiting for the first ping.".to_string()
        }
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
    /// The monitor a create form was copied from, checked to exist.
    pub copy_of: Option<i64>,
    /// Every channel, sorted by name, for the alert checkboxes.
    pub channels: Vec<Channel>,
    /// The groups in use, suggested by the group field.
    pub groups: Vec<String>,
    /// What the Test button found: a summary, or why it failed.
    pub test_result: Option<Result<String, String>>,
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

    pub fn is_push(&self) -> bool {
        self.form.kind == PUSH
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::verdict::{ErrorKind, Failure};
    use crate::views::heartbeat::Tick;

    fn passed(tls_expires_at: Option<i64>) -> CheckOutcome {
        CheckOutcome {
            checked_at: 100 * DAY_MS,
            latency_ms: Some(84),
            status_code: Some(200),
            failure: None,
            tls_expires_at,
        }
    }

    #[test]
    fn certificate_label_says_expired_and_counts_days() {
        assert_eq!(certificate_label(10 * DAY_MS - 1, 10 * DAY_MS), "Expired");
        assert_eq!(certificate_label(11 * DAY_MS, 10 * DAY_MS), "1 day left");
        assert_eq!(certificate_label(15 * DAY_MS, 10 * DAY_MS), "5 days left");
    }

    #[test]
    fn test_summary_gives_response_certificate_and_coming_warning() {
        assert_eq!(
            test_summary(&passed(None), Some(14)),
            Ok("HTTP 200 in 84 ms.".to_string())
        );
        assert_eq!(
            test_summary(&passed(Some(150 * DAY_MS)), Some(14)),
            Ok("HTTP 200 in 84 ms. Certificate: 50 days left.".to_string())
        );
        assert_eq!(
            test_summary(&passed(Some(105 * DAY_MS)), Some(14)),
            Ok(
                "HTTP 200 in 84 ms. Certificate: 5 days left, within the 14-day warning: \
                saving sends a certificate alert."
                    .to_string()
            )
        );
        assert_eq!(
            test_summary(&passed(Some(105 * DAY_MS)), None),
            Ok("HTTP 200 in 84 ms. Certificate: 5 days left.".to_string())
        );
        assert_eq!(
            test_summary(&passed(Some(99 * DAY_MS)), None),
            Ok("HTTP 200 in 84 ms. Certificate: Expired.".to_string())
        );
        let failed = CheckOutcome {
            failure: Some(Failure {
                kind: ErrorKind::Status,
                message: "HTTP 500, expected 200-299".to_string(),
            }),
            ..passed(None)
        };
        assert_eq!(
            test_summary(&failed, Some(14)),
            Err("HTTP 500, expected 200-299".to_string())
        );
    }

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
