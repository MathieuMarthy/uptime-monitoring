//! Configuration export and import: a JSON document that links entities by name, so it can be
//! imported into another instance. History (checks, incidents, stats) and the account are left out.

use crate::models::{Channel, Maintenance, Monitor, StatusPage};
use crate::time::now_ms;
use crate::windows::Schedule;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;

pub mod import;
pub mod kuma;

/// The `format` field of an export.
pub const FORMAT: &str = "uptime-monitoring";
/// Bumped when a change to the document cannot be read by older importers.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub exported_at: i64,
    #[serde(default)]
    pub channels: Vec<ChannelExport>,
    #[serde(default)]
    pub monitors: Vec<MonitorExport>,
    #[serde(default)]
    pub status_pages: Vec<StatusPageExport>,
    #[serde(default)]
    pub maintenances: Vec<MaintenanceExport>,
}

/// A notification channel, its secrets included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelExport {
    pub name: String,
    pub kind: String,
    /// As stored: `{url}`, `{server, topic, token?}` or `{token, chat_id, server?}`.
    pub config: serde_json::Value,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub group_alerts: bool,
}

/// A monitor; missing fields take the defaults of a new monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorExport {
    pub name: String,
    pub group: Option<String>,
    pub kind: String,
    pub url: String,
    pub method: String,
    pub headers: Option<String>,
    pub body: Option<String>,
    pub keyword: Option<String>,
    pub accepted_statuses: String,
    pub follow_redirects: bool,
    pub ignore_tls_errors: bool,
    pub interval_s: i64,
    pub retry_interval_s: i64,
    pub timeout_s: i64,
    pub failure_threshold: i64,
    pub tls_expiry_warn_days: Option<i64>,
    pub grace_s: i64,
    pub latency_warn_ms: Option<i64>,
    pub latency_alerts: bool,
    pub paused: bool,
    /// Push monitors: kept so that the monitored jobs keep their URL.
    pub push_token: Option<String>,
    /// Channel names.
    pub channels: Vec<String>,
}

impl Default for MonitorExport {
    fn default() -> Self {
        Self {
            name: String::new(),
            group: None,
            kind: "http".to_string(),
            url: String::new(),
            method: "GET".to_string(),
            headers: None,
            body: None,
            keyword: None,
            accepted_statuses: "200-299".to_string(),
            follow_redirects: true,
            ignore_tls_errors: false,
            interval_s: 60,
            retry_interval_s: 20,
            timeout_s: 10,
            failure_threshold: 3,
            tls_expiry_warn_days: Some(14),
            grace_s: 60,
            latency_warn_ms: None,
            latency_alerts: true,
            paused: false,
            push_token: None,
            channels: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusPageExport {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub published: bool,
    /// In display order.
    #[serde(default)]
    pub monitors: Vec<PageMonitorExport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageMonitorExport {
    /// The monitor's name.
    pub monitor: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaintenanceExport {
    pub title: String,
    pub schedule: ScheduleExport,
    #[serde(default)]
    pub all_monitors: bool,
    #[serde(default)]
    pub show_on_status_pages: bool,
    /// Monitor names; ignored when `all_monitors`.
    #[serde(default)]
    pub monitors: Vec<String>,
}

/// As stored: epoch milliseconds, or a weekday mask (bit 0 = Monday) and minutes after local
/// midnight.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScheduleExport {
    Once {
        starts_at: i64,
        ends_at: i64,
    },
    Weekly {
        weekdays: u8,
        local_start: i64,
        duration_min: i64,
    },
}

fn yes() -> bool {
    true
}

impl From<Schedule> for ScheduleExport {
    fn from(schedule: Schedule) -> Self {
        match schedule {
            Schedule::Once { starts_at, ends_at } => Self::Once { starts_at, ends_at },
            Schedule::Weekly {
                weekdays,
                local_start,
                duration_min,
            } => Self::Weekly {
                weekdays,
                local_start,
                duration_min,
            },
        }
    }
}

impl Export {
    /// An empty document of the current version.
    pub fn new() -> Self {
        Self {
            format: FORMAT.to_string(),
            version: VERSION,
            app_version: crate::VERSION.to_string(),
            exported_at: now_ms(),
            channels: Vec::new(),
            monitors: Vec::new(),
            status_pages: Vec::new(),
            maintenances: Vec::new(),
        }
    }

    /// The whole configuration of the instance.
    pub async fn load(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        let mut export = Self::new();

        let channels = Channel::all(pool).await?;
        let channel_names: HashMap<i64, &str> =
            channels.iter().map(|c| (c.id, c.name.as_str())).collect();
        let mut links: HashMap<i64, Vec<String>> = HashMap::new();
        for (monitor_id, channel_id) in Channel::links(pool).await? {
            if let Some(name) = channel_names.get(&channel_id) {
                links.entry(monitor_id).or_default().push(name.to_string());
            }
        }
        links.values_mut().for_each(|names| names.sort());
        export.channels = channels
            .iter()
            .map(|c| ChannelExport {
                name: c.name.clone(),
                kind: c.kind.clone(),
                config: serde_json::from_str(&c.config).unwrap_or(serde_json::Value::Null),
                enabled: c.enabled,
                group_alerts: c.group_alerts,
            })
            .collect();

        let monitors = Monitor::find_all(pool).await?;
        let monitor_names: HashMap<i64, &str> =
            monitors.iter().map(|m| (m.id, m.name.as_str())).collect();
        export.monitors = monitors
            .iter()
            .map(|m| MonitorExport {
                name: m.name.clone(),
                group: m.group_name.clone(),
                kind: m.kind.clone(),
                url: m.url.clone(),
                method: m.method.clone(),
                headers: m.headers.clone(),
                body: m.body.clone(),
                keyword: m.keyword.clone(),
                accepted_statuses: m.accepted_statuses.clone(),
                follow_redirects: m.follow_redirects,
                ignore_tls_errors: m.ignore_tls_errors,
                interval_s: m.interval_s,
                retry_interval_s: m.retry_interval_s,
                timeout_s: m.timeout_s,
                failure_threshold: m.failure_threshold,
                tls_expiry_warn_days: m.tls_expiry_warn_days,
                grace_s: m.grace_s,
                latency_warn_ms: m.latency_warn_ms,
                latency_alerts: m.latency_alerts,
                paused: m.paused,
                push_token: m.push_token.clone(),
                channels: links.remove(&m.id).unwrap_or_default(),
            })
            .collect();
        let named = |ids: &[i64]| -> Vec<String> {
            ids.iter()
                .filter_map(|id| monitor_names.get(id).map(|n| n.to_string()))
                .collect()
        };

        for page in StatusPage::all(pool).await? {
            let monitors = StatusPage::monitors(pool, page.id)
                .await?
                .into_iter()
                .filter_map(|m| {
                    Some(PageMonitorExport {
                        monitor: monitor_names.get(&m.monitor_id)?.to_string(),
                        display_name: m.display_name,
                    })
                })
                .collect();
            export.status_pages.push(StatusPageExport {
                slug: page.slug,
                title: page.title,
                description: page.description,
                published: page.published,
                monitors,
            });
        }

        export.maintenances = Maintenance::all(pool)
            .await?
            .into_iter()
            .map(|(m, ids)| MaintenanceExport {
                schedule: m.schedule().into(),
                title: m.title,
                all_monitors: m.all_monitors,
                show_on_status_pages: m.show_on_status_pages,
                monitors: if m.all_monitors {
                    Vec::new()
                } else {
                    let mut names = named(&ids);
                    names.sort();
                    names
                },
            })
            .collect();

        Ok(export)
    }
}

impl Default for Export {
    fn default() -> Self {
        Self::new()
    }
}
