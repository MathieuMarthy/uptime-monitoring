//! Import of an export document: only adds. Every entry goes through the validation of its admin
//! form; an entry whose name (or slug) is taken, or that is invalid, is skipped with its reason.

use super::{
    ChannelExport, Export, FORMAT, MaintenanceExport, MonitorExport, ScheduleExport,
    StatusPageExport, VERSION, kuma,
};
use crate::forms::channel::ChannelForm;
use crate::forms::maintenance::{MaintenanceForm, format_local};
use crate::forms::monitor::{FieldErrors, MonitorForm};
use crate::forms::status_page::{PageMonitorInput, StatusPageForm};
use crate::models::{Channel, Maintenance, Monitor, StatusPage, name_key};
use crate::notify::channel::ChannelConfig;
use crate::windows::Schedule;
use jiff::tz::TimeZone;
use sqlx::{SqliteConnection, SqlitePool};
use std::collections::{HashMap, HashSet};

/// What an import did.
#[derive(Debug, Default)]
pub struct Report {
    pub added: usize,
    pub skipped: Vec<Skipped>,
    /// Added, but something needs the admin's attention.
    pub notices: Vec<String>,
    /// The monitors added, for the check engine.
    pub monitor_ids: Vec<i64>,
}

/// An entry left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// `Monitor`, `Channel`, `Public page` or `Maintenance`.
    pub kind: &'static str,
    pub name: String,
    pub reason: String,
}

impl Skipped {
    pub fn new(kind: &'static str, name: &str, reason: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.to_string(),
            reason: reason.into(),
        }
    }
}

const EXISTS: &str = "already exists.";

/// Reads an export of this application or an Uptime Kuma backup. Kuma entries that cannot be
/// converted are already in the report.
pub fn parse(text: &str) -> Result<(Export, Report), String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "This is not a JSON document.".to_string())?;
    if value.get("format").and_then(|f| f.as_str()) == Some(FORMAT) {
        let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        if version == 0 || version > u64::from(VERSION) {
            return Err(
                "This file comes from a newer version of Uptime Monitoring: update first."
                    .to_string(),
            );
        }
        let export = serde_json::from_value(value)
            .map_err(|e| format!("This export cannot be read: {e}."))?;
        Ok((export, Report::default()))
    } else if value.get("monitorList").is_some() {
        Ok(kuma::convert(&value))
    } else {
        Err("Neither an export of Uptime Monitoring nor an Uptime Kuma backup.".to_string())
    }
}

/// What is already stored, read before the transaction: the pool may have a single connection.
struct Existing {
    /// Channel name in ASCII lowercase, as `COLLATE NOCASE` compares it → id.
    channels: HashMap<String, i64>,
    /// Monitor name key → id.
    monitors: HashMap<String, i64>,
    /// Every monitor's id and group, for maintenance windows.
    groups: Vec<(i64, Option<String>)>,
    push_tokens: HashSet<String>,
    slugs: HashSet<String>,
    maintenances: HashSet<String>,
}

impl Existing {
    async fn load(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        let monitors = Monitor::find_all(pool).await?;
        Ok(Self {
            channels: Channel::names(pool)
                .await?
                .into_iter()
                .map(|(id, name)| (name.to_ascii_lowercase(), id))
                .collect(),
            monitors: monitors.iter().map(|m| (name_key(&m.name), m.id)).collect(),
            groups: monitors
                .iter()
                .map(|m| (m.id, m.group_name.clone()))
                .collect(),
            push_tokens: monitors
                .iter()
                .filter_map(|m| m.push_token.clone())
                .collect(),
            slugs: StatusPage::all(pool)
                .await?
                .into_iter()
                .map(|p| p.slug)
                .collect(),
            maintenances: Maintenance::all(pool)
                .await?
                .into_iter()
                .map(|(m, _)| name_key(&m.title))
                .collect(),
        })
    }
}

/// Adds what the document holds, in one transaction; `tz` reads maintenance times.
pub async fn apply(
    pool: &SqlitePool,
    tz: &TimeZone,
    export: &Export,
    report: &mut Report,
) -> Result<(), sqlx::Error> {
    let mut existing = Existing::load(pool).await?;
    let mut tx = pool.begin().await?;
    for channel in &export.channels {
        add_channel(&mut tx, channel, &mut existing, report).await?;
    }
    for monitor in &export.monitors {
        add_monitor(&mut tx, monitor, &mut existing, report).await?;
    }
    for page in &export.status_pages {
        add_page(&mut tx, page, &mut existing, report).await?;
    }
    for maintenance in &export.maintenances {
        add_maintenance(&mut tx, tz, maintenance, &mut existing, report).await?;
    }
    tx.commit().await
}

fn reason(errors: FieldErrors) -> String {
    errors.into_values().collect::<Vec<_>>().join(" ")
}

async fn add_channel(
    conn: &mut SqliteConnection,
    channel: &ChannelExport,
    existing: &mut Existing,
    report: &mut Report,
) -> Result<(), sqlx::Error> {
    let skip = |reason: &str| Skipped::new("Channel", &channel.name, reason);
    let key = channel.name.trim().to_ascii_lowercase();
    if existing.channels.contains_key(&key) {
        report.skipped.push(skip(EXISTS));
        return Ok(());
    }
    let Ok(config) = ChannelConfig::parse(&channel.kind, &channel.config.to_string()) else {
        report.skipped.push(skip("unknown kind or settings."));
        return Ok(());
    };
    let mut form = ChannelForm::from_config(
        &channel.name,
        &config,
        channel.enabled,
        channel.group_alerts,
    );
    match &config {
        ChannelConfig::Ntfy(c) => form.ntfy_token = c.token.clone().unwrap_or_default(),
        ChannelConfig::Telegram(c) => form.telegram_token = c.token.clone(),
        ChannelConfig::Webhook(_) | ChannelConfig::Discord(_) => {}
    }
    match form.validate(None) {
        Ok(input) => {
            let id = Channel::insert(conn, &input).await?;
            existing.channels.insert(key, id);
            report.added += 1;
        }
        Err(errors) => report.skipped.push(skip(&reason(errors))),
    }
    Ok(())
}

fn monitor_form(m: &MonitorExport, channel_ids: &[i64]) -> MonitorForm {
    let on = |b: bool| b.then(|| "on".to_string());
    let number = |n: Option<i64>| n.map(|n| n.to_string()).unwrap_or_default();
    let mut form = MonitorForm {
        name: m.name.clone(),
        group: m.group.clone().unwrap_or_default(),
        kind: m.kind.clone(),
        url: m.url.clone(),
        method: m.method.clone(),
        accepted_statuses: m.accepted_statuses.clone(),
        interval_s: m.interval_s.to_string(),
        retry_interval_s: m.retry_interval_s.to_string(),
        timeout_s: m.timeout_s.to_string(),
        failure_threshold: m.failure_threshold.to_string(),
        tls_expiry_warn_days: number(m.tls_expiry_warn_days),
        grace_s: m.grace_s.to_string(),
        latency_warn_ms: number(m.latency_warn_ms),
        latency_alerts: on(m.latency_alerts),
        headers: m.headers.clone().unwrap_or_default(),
        body: m.body.clone().unwrap_or_default(),
        keyword: m.keyword.clone().unwrap_or_default(),
        follow_redirects: on(m.follow_redirects),
        ignore_tls_errors: on(m.ignore_tls_errors),
        active: on(!m.paused),
        ..MonitorForm::default()
    };
    form.set_channels(channel_ids);
    form
}

/// A token as this application makes them: 64 lowercase hex digits.
fn is_push_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

async fn add_monitor(
    conn: &mut SqliteConnection,
    monitor: &MonitorExport,
    existing: &mut Existing,
    report: &mut Report,
) -> Result<(), sqlx::Error> {
    let skip = |reason: &str| Skipped::new("Monitor", &monitor.name, reason);
    let key = name_key(monitor.name.trim());
    if existing.monitors.contains_key(&key) {
        report.skipped.push(skip(EXISTS));
        return Ok(());
    }
    let channel_ids: Vec<i64> = monitor
        .channels
        .iter()
        .filter_map(|name| existing.channels.get(&name.trim().to_ascii_lowercase()))
        .copied()
        .collect();
    let input = match monitor_form(monitor, &channel_ids).validate() {
        Ok(input) => input,
        Err(errors) => {
            report.skipped.push(skip(&reason(errors)));
            return Ok(());
        }
    };
    let id = Monitor::insert(conn, &input).await?;
    if input.kind == crate::forms::monitor::PUSH {
        let kept = match monitor.push_token.as_deref() {
            Some(token) if is_push_token(token) && !existing.push_tokens.contains(token) => {
                Monitor::set_push_token(conn, id, token).await?
            }
            _ => false,
        };
        if !kept {
            report.notices.push(format!(
                "Monitor “{}” has a new push URL: update the job that calls it.",
                input.name
            ));
        }
    }
    existing.monitors.insert(key, id);
    existing.groups.push((id, input.group_name.clone()));
    report.monitor_ids.push(id);
    report.added += 1;
    Ok(())
}

async fn add_page(
    conn: &mut SqliteConnection,
    page: &StatusPageExport,
    existing: &mut Existing,
    report: &mut Report,
) -> Result<(), sqlx::Error> {
    let skip = |reason: &str| Skipped::new("Public page", &page.slug, reason);
    let slug = page.slug.trim();
    if existing.slugs.contains(slug) {
        report.skipped.push(skip(EXISTS));
        return Ok(());
    }
    let mut seen = HashSet::new();
    let monitors: Vec<PageMonitorInput> = page
        .monitors
        .iter()
        .filter_map(|m| {
            let monitor_id = *existing.monitors.get(&name_key(m.monitor.trim()))?;
            seen.insert(monitor_id).then(|| PageMonitorInput {
                monitor_id,
                display_name: m.display_name.clone(),
            })
        })
        .collect();
    let order: Vec<i64> = monitors.iter().map(|m| m.monitor_id).collect();
    let form = StatusPageForm::from_page(
        &page.title,
        &page.slug,
        page.description.as_deref(),
        page.published,
        &monitors,
    );
    match form.validate(&order) {
        Ok(input) => {
            StatusPage::insert(conn, &input).await?;
            existing.slugs.insert(input.slug);
            report.added += 1;
        }
        Err(errors) => report.skipped.push(skip(&reason(errors))),
    }
    Ok(())
}

async fn add_maintenance(
    conn: &mut SqliteConnection,
    tz: &TimeZone,
    maintenance: &MaintenanceExport,
    existing: &mut Existing,
    report: &mut Report,
) -> Result<(), sqlx::Error> {
    let skip = |reason: &str| Skipped::new("Maintenance", &maintenance.title, reason);
    let key = name_key(maintenance.title.trim());
    if existing.maintenances.contains(&key) {
        report.skipped.push(skip(EXISTS));
        return Ok(());
    }
    let on = |b: bool| b.then(|| "on".to_string());
    let mut form = MaintenanceForm {
        title: maintenance.title.clone(),
        all_monitors: on(maintenance.all_monitors),
        show_on_status_pages: on(maintenance.show_on_status_pages),
        ..MaintenanceForm::new_defaults()
    };
    match maintenance.schedule {
        ScheduleExport::Once { starts_at, ends_at } => {
            form.starts_at = format_local(tz, starts_at);
            form.ends_at = format_local(tz, ends_at);
        }
        ScheduleExport::Weekly {
            weekdays,
            local_start,
            duration_min,
        } => {
            form.kind = "weekly".to_string();
            for day in (0..7).filter(|day| weekdays & (1 << day) != 0) {
                form.other.insert(format!("day_{day}"), "on".to_string());
            }
            if (0..24 * 60).contains(&local_start) {
                form.local_start = format!("{:02}:{:02}", local_start / 60, local_start % 60);
            }
            form.duration_min = duration_min.to_string();
        }
    }
    for name in &maintenance.monitors {
        if let Some(id) = existing.monitors.get(&name_key(name.trim())) {
            form.other.insert(format!("monitor_{id}"), "on".to_string());
        }
    }
    match form.validate(tz, &existing.groups) {
        Ok(mut input) => {
            // The form works in minutes; the export keeps the exact times.
            if let ScheduleExport::Once { starts_at, ends_at } = maintenance.schedule {
                input.schedule = Schedule::Once { starts_at, ends_at };
            }
            Maintenance::insert(conn, &input).await?;
            existing.maintenances.insert(key);
            report.added += 1;
        }
        Err(errors) => report.skipped.push(skip(&reason(errors))),
    }
    Ok(())
}
