//! The Activity page: outages, failed checks and certificate warnings across monitors, latest
//! first, grouped by local day.

use crate::error::AppError;
use crate::models::{
    CertificateWarning, Check, FailedCheck, Incident, Monitor, MonitorState, MonitorSummary,
    NamedIncident, NotificationLog, Settings,
};
use crate::time::{DAY_MS, day_of, days_left, format_datetime, format_time, now_ms};
use crate::views::incidents::{
    ALERT_DELAY_MS, AlertRow, Context, Fact, FailureRow, Following, IncidentDetail,
    error_kind_label,
};
use askama::Template;
use askama_web::WebTemplate;
use jiff::ToSpan;
use jiff::tz::TimeZone;
use sqlx::SqlitePool;

/// Events per page.
pub const PAGE: usize = 50;
const PERIODS: [i64; 3] = [7, 30, 90];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    All,
    Outages,
    Checks,
    Certificates,
}

impl Kind {
    pub const ALL: [Self; 4] = [Self::All, Self::Outages, Self::Checks, Self::Certificates];

    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("outages") => Self::Outages,
            Some("checks") => Self::Checks,
            Some("certificates") => Self::Certificates,
            _ => Self::All,
        }
    }

    fn param(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Outages => Some("outages"),
            Self::Checks => Some("checks"),
            Self::Certificates => Some("certificates"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All events",
            Self::Outages => "Outages",
            Self::Checks => "Failed checks",
            Self::Certificates => "Certificates",
        }
    }

    fn shows(self, other: Self) -> bool {
        self == Self::All || self == other
    }
}

/// The page's query string, read leniently: anything unexpected falls back to the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filter {
    pub kind: Kind,
    pub monitor: Option<i64>,
    pub days: i64,
    /// Only events after this one, latest first: the next page.
    pub before: Option<Cursor>,
}

/// The source of an event, which orders events logged at the same millisecond.
const OUTAGE: u8 = 2;
const FAILURE: u8 = 1;
const CERTIFICATE: u8 = 0;

/// Where a page ends. Events sort by time, then source, then id (the incident's, or the
/// monitor's for the other sources), latest first, so two events never tie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cursor {
    pub at: i64,
    pub source: u8,
    pub id: i64,
}

impl Cursor {
    /// `at-source-id`.
    fn parse(value: &str) -> Option<Self> {
        let mut parts = value.splitn(3, '-').map(str::parse::<i64>);
        let (Some(Ok(at)), Some(Ok(source)), Some(Ok(id))) =
            (parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        Some(Self {
            at,
            source: u8::try_from(source).ok()?,
            id,
        })
    }

    /// For one source's query: events before `at`, or at `at` with an id below the second value.
    fn bounds(cursor: Option<Self>, source: u8) -> (i64, i64) {
        match cursor {
            None => (i64::MAX, i64::MAX),
            Some(c) if source < c.source => (c.at, i64::MAX),
            Some(c) if source == c.source => (c.at, c.id),
            Some(c) => (c.at, i64::MIN),
        }
    }
}

impl std::fmt::Display for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}-{}", self.at, self.source, self.id)
    }
}

impl Filter {
    pub fn parse(
        kind: Option<&str>,
        monitor: Option<&str>,
        days: Option<&str>,
        before: Option<&str>,
    ) -> Self {
        let days = days
            .and_then(|d| d.parse().ok())
            .filter(|d| PERIODS.contains(d))
            .unwrap_or(PERIODS[0]);
        Self {
            kind: Kind::parse(kind),
            monitor: monitor.and_then(|m| m.parse().ok()).filter(|&m| m > 0),
            days,
            before: before.and_then(Cursor::parse),
        }
    }

    /// `/activity` with this filter, the kind and cursor replaced.
    fn url(self, kind: Kind, before: Option<Cursor>) -> String {
        let mut params = Vec::new();
        if let Some(kind) = kind.param() {
            params.push(format!("type={kind}"));
        }
        if let Some(monitor) = self.monitor {
            params.push(format!("monitor={monitor}"));
        }
        if self.days != PERIODS[0] {
            params.push(format!("days={}", self.days));
        }
        if let Some(before) = before {
            params.push(format!("before={before}"));
        }
        if params.is_empty() {
            "/activity".to_string()
        } else {
            format!("/activity?{}", params.join("&"))
        }
    }
}

pub struct FailureEvent {
    pub monitor_id: i64,
    pub monitor_name: String,
    pub time: String,
    pub error: String,
    pub kind: &'static str,
    pub facts: Vec<Fact>,
    pub row: FailureRow,
}

pub struct CertificateEvent {
    pub monitor_id: i64,
    pub monitor_name: String,
    pub time: String,
    pub expires: String,
    pub renewed: bool,
    pub tag: String,
    pub facts: Vec<Fact>,
    pub alerts: Vec<AlertRow>,
}

pub enum Event {
    Outage(IncidentDetail),
    Failure(FailureEvent),
    Certificate(CertificateEvent),
}

pub struct Day {
    pub label: String,
    pub events: Vec<Event>,
}

pub struct KindLink {
    pub label: &'static str,
    pub url: String,
    pub current: bool,
}

#[derive(Template, WebTemplate)]
#[template(path = "activity/index.html")]
pub struct IndexTemplate {
    pub filter: Filter,
    pub kinds: Vec<KindLink>,
    pub monitors: Vec<MonitorSummary>,
    pub periods: [i64; 3],
    pub days: Vec<Day>,
    pub older_url: Option<String>,
    pub checks_retention_days: i64,
    pub csrf_token: String,
}

/// One source's row, before it is turned into an event.
enum Raw {
    Outage(NamedIncident),
    Failure(FailedCheck),
    Certificate(CertificateWarning),
}

impl Raw {
    fn cursor(&self) -> Cursor {
        let (at, source, id) = match self {
            Self::Outage(i) => (i.incident.started_at, OUTAGE, i.incident.id),
            Self::Failure(f) => (f.checked_at, FAILURE, f.monitor_id),
            Self::Certificate(w) => (w.warned_at, CERTIFICATE, w.monitor_id),
        };
        Cursor { at, source, id }
    }
}

impl IndexTemplate {
    pub async fn load(
        pool: &SqlitePool,
        filter: Filter,
        csrf_token: String,
    ) -> Result<Self, AppError> {
        let settings = Settings::load(pool).await?;
        let tz = settings.tz();
        let now = now_ms();
        let from = now - filter.days * DAY_MS;
        let limit = PAGE as i64 + 1;
        let (monitor, after) = (filter.monitor, filter.before);

        let mut raw = Vec::new();
        if filter.kind.shows(Kind::Outages) {
            let (before, tie) = Cursor::bounds(after, OUTAGE);
            for i in Incident::started_between(pool, monitor, from, before, tie, limit).await? {
                raw.push(Raw::Outage(i));
            }
        }
        if filter.kind.shows(Kind::Checks) {
            let (before, tie) = Cursor::bounds(after, FAILURE);
            for f in Check::isolated_failures(pool, monitor, from, before, tie, limit).await? {
                raw.push(Raw::Failure(f));
            }
        }
        if filter.kind.shows(Kind::Certificates) {
            let (before, tie) = Cursor::bounds(after, CERTIFICATE);
            for w in MonitorState::warnings_between(pool, monitor, from, before, tie, limit).await?
            {
                raw.push(Raw::Certificate(w));
            }
        }
        raw.sort_by_key(|r| std::cmp::Reverse(r.cursor()));
        let older_url =
            (raw.len() > PAGE).then(|| filter.url(filter.kind, Some(raw[PAGE - 1].cursor())));
        raw.truncate(PAGE);

        let mut days: Vec<Day> = Vec::new();
        for r in raw {
            let label = day_label(&tz, r.cursor().at, now);
            let event = match r {
                Raw::Outage(i) => {
                    let following = i.next_started_at.map(|started_at| Following {
                        started_at,
                        resolved_at: i.next_resolved_at,
                    });
                    Event::Outage(
                        IncidentDetail::load(
                            pool,
                            &tz,
                            i.incident,
                            &i.monitor_name,
                            following,
                            Context::Activity,
                            now,
                        )
                        .await?,
                    )
                }
                Raw::Failure(f) => Event::Failure(failure_event(&tz, f)),
                Raw::Certificate(w) => {
                    Event::Certificate(certificate_event(pool, &tz, w, now).await?)
                }
            };
            match days.last_mut() {
                Some(day) if day.label == label => day.events.push(event),
                _ => days.push(Day {
                    label,
                    events: vec![event],
                }),
            }
        }

        let kinds = Kind::ALL
            .into_iter()
            .map(|kind| KindLink {
                label: kind.label(),
                url: filter.url(kind, None),
                current: kind == filter.kind,
            })
            .collect();

        Ok(Self {
            filter,
            kinds,
            monitors: Monitor::summaries(pool).await?,
            periods: PERIODS,
            days,
            older_url,
            checks_retention_days: settings.checks_retention_days,
            csrf_token,
        })
    }

    pub fn is_selected(&self, monitor: &i64) -> bool {
        self.filter.monitor == Some(*monitor)
    }

    /// What the page lists, for its lede and empty state.
    pub fn scope(&self) -> &'static str {
        match self.filter.kind {
            Kind::All => "Outages, failed checks and certificate warnings",
            Kind::Outages => "Outages",
            Kind::Checks => "Failed checks that did not make a monitor down",
            Kind::Certificates => "Certificate warnings",
        }
    }
}

fn failure_event(tz: &TimeZone, f: FailedCheck) -> FailureEvent {
    let row = FailureRow::new(tz, &f);
    let kind = error_kind_label(f.error_kind.as_deref());
    FailureEvent {
        monitor_id: f.monitor_id,
        time: format_time(tz, f.checked_at),
        error: row.error.clone(),
        kind,
        facts: vec![
            Fact {
                label: "Checked",
                value: row.time.clone(),
            },
            Fact {
                label: "Error type",
                value: kind.to_string(),
            },
            Fact {
                label: "HTTP status",
                value: f
                    .status_code
                    .map_or_else(|| "—".to_string(), |c| c.to_string()),
            },
            Fact {
                label: "Response",
                value: f
                    .latency_ms
                    .map_or_else(|| "—".to_string(), |ms| format!("{ms} ms")),
            },
        ],
        monitor_name: f.monitor_name,
        row,
    }
}

async fn certificate_event(
    pool: &SqlitePool,
    tz: &TimeZone,
    w: CertificateWarning,
    now: i64,
) -> Result<CertificateEvent, AppError> {
    let alerts = NotificationLog::for_monitor_between(
        pool,
        w.monitor_id,
        &["tls_expiring"],
        w.warned_at,
        w.warned_at + ALERT_DELAY_MS,
    )
    .await?;
    let renewed = w.current_expiry.is_some_and(|at| at > w.expires_at);
    let tag = match days_left(w.expires_at, now) {
        _ if renewed => "Renewed".to_string(),
        left if left < 0 => "Expired".to_string(),
        1 => "1 day left".to_string(),
        left => format!("{left} days left"),
    };
    let mut facts = vec![
        Fact {
            label: "Warned",
            value: format_datetime(tz, w.warned_at),
        },
        Fact {
            label: "Expires",
            value: format_datetime(tz, w.expires_at),
        },
    ];
    if let Some(current) = w.current_expiry.filter(|_| renewed) {
        facts.push(Fact {
            label: "New certificate expires",
            value: format_datetime(tz, current),
        });
    }
    Ok(CertificateEvent {
        monitor_id: w.monitor_id,
        monitor_name: w.monitor_name,
        time: format_time(tz, w.warned_at),
        expires: day_of(tz, w.expires_at).strftime("%-d %B %Y").to_string(),
        renewed,
        tag,
        facts,
        alerts: alerts.into_iter().map(|a| AlertRow::new(tz, a)).collect(),
    })
}

/// `Today, Thursday 29 October`, `Yesterday, …`, then `Monday 26 October`.
fn day_label(tz: &TimeZone, at: i64, now: i64) -> String {
    let day = day_of(tz, at);
    let today = day_of(tz, now);
    let name = day.strftime("%A %-d %B").to_string();
    if day == today {
        format!("Today, {name}")
    } else if today.checked_sub(1.day()).ok() == Some(day) {
        format!("Yesterday, {name}")
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_falls_back_to_defaults() {
        let f = Filter::parse(Some("nope"), Some("-1"), Some("12"), Some("x"));
        assert_eq!(
            f,
            Filter {
                kind: Kind::All,
                monitor: None,
                days: 7,
                before: None
            }
        );
        let f = Filter::parse(Some("checks"), Some("3"), Some("30"), Some("99-1-4"));
        assert_eq!(
            f.url(f.kind, f.before),
            "/activity?type=checks&monitor=3&days=30&before=99-1-4"
        );
        assert_eq!(Filter::parse(None, None, None, Some("99")).before, None);
        assert_eq!(f.url(Kind::All, None), "/activity?monitor=3&days=30");
    }
}
