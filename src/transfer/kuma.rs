//! Uptime Kuma backups (the JSON of Settings › Backup in Kuma 1.x): its HTTP, keyword and push
//! monitors and its webhook, Discord, ntfy and Telegram notifications, turned into an export.
//! Kuma writes booleans as `0`/`1` or `true`/`false` and numbers as integers or floats: every
//! field is read leniently.

use super::import::{Report, Skipped};
use super::{ChannelExport, Export, MonitorExport};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// Kuma's default certificate warning, the first of its 7, 14 and 21 days.
const TLS_WARN_DAYS: i64 = 14;

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn number(v: &Value, key: &str) -> Option<i64> {
    let n = v.get(key)?;
    n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64))
}

fn flag(v: &Value, key: &str) -> Option<bool> {
    match v.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => Some(n.as_f64() != Some(0.0)),
        _ => None,
    }
}

fn id(v: &Value) -> Option<i64> {
    number(v, "id")
}

pub fn convert(backup: &Value) -> (Export, Report) {
    let mut export = Export::new();
    let mut report = Report::default();
    let empty = Vec::new();

    let mut channel_names: HashMap<i64, String> = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    let notifications = backup.get("notificationList").and_then(Value::as_array);
    for notification in notifications.unwrap_or(&empty) {
        // Kuma allows twin names; here a name is unique and links monitors to it.
        let base = text(notification, "name").unwrap_or_default();
        let mut name = base.clone();
        for n in 2.. {
            if taken.insert(name.to_ascii_lowercase()) {
                break;
            }
            name = format!("{base} ({n})");
        }
        match channel(notification, &name) {
            Ok(channel) => {
                if let Some(id) = id(notification) {
                    channel_names.insert(id, name);
                }
                export.channels.push(channel);
            }
            Err(reason) => report.skipped.push(Skipped::new("Channel", &name, reason)),
        }
    }

    let monitors = backup
        .get("monitorList")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let groups: HashMap<i64, String> = monitors
        .iter()
        .filter(|m| text(m, "type").as_deref() == Some("group"))
        .filter_map(|m| Some((id(m)?, text(m, "name")?)))
        .collect();
    for m in monitors {
        let name = text(m, "name").unwrap_or_default();
        match monitor(m, &groups, &channel_names) {
            Ok(Some(monitor)) => export.monitors.push(monitor),
            Ok(None) => {}
            Err(reason) => report.skipped.push(Skipped::new("Monitor", &name, reason)),
        }
    }
    (export, report)
}

fn channel(notification: &Value, name: &str) -> Result<ChannelExport, String> {
    // The settings are a JSON document inside a string.
    let config: Value = match notification.get("config") {
        Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or(Value::Null),
        Some(other) => other.clone(),
        None => Value::Null,
    };
    let kind = text(&config, "type").unwrap_or_default();
    let (kind, config) = match kind.as_str() {
        "webhook" => (
            "webhook",
            json!({"url": text(&config, "webhookURL").unwrap_or_default()}),
        ),
        "discord" => (
            "discord",
            json!({"url": text(&config, "discordWebhookUrl").unwrap_or_default()}),
        ),
        "ntfy" => {
            let token = text(&config, "ntfyaccesstoken");
            if token.is_none() && text(&config, "ntfyusername").is_some() {
                return Err("ntfy login with a user name and password is not supported.".into());
            }
            let mut ntfy = json!({
                "server": text(&config, "ntfyserverurl").unwrap_or_else(|| "https://ntfy.sh".into()),
                "topic": text(&config, "ntfytopic").unwrap_or_default(),
            });
            if let Some(token) = token {
                ntfy["token"] = json!(token);
            }
            ("ntfy", ntfy)
        }
        "telegram" => (
            "telegram",
            json!({
                "token": text(&config, "telegramBotToken").unwrap_or_default(),
                "chat_id": text(&config, "telegramChatID").unwrap_or_default(),
            }),
        ),
        "" => return Err("no notification type.".into()),
        other => return Err(format!("“{other}” notifications are not supported.")),
    };
    Ok(ChannelExport {
        name: name.to_string(),
        kind: kind.to_string(),
        config,
        enabled: flag(notification, "active").unwrap_or(true),
        group_alerts: true,
    })
}

/// `None` for a group: it becomes the group name of its monitors.
fn monitor(
    m: &Value,
    groups: &HashMap<i64, String>,
    channel_names: &HashMap<i64, String>,
) -> Result<Option<MonitorExport>, String> {
    let kind = text(m, "type").unwrap_or_default();
    match kind.as_str() {
        "group" => return Ok(None),
        "http" | "push" => {}
        "keyword" if flag(m, "invertKeyword") == Some(true) => {
            return Err("an inverted keyword is not supported.".into());
        }
        "keyword" => {}
        other => return Err(format!("“{other}” monitors are not supported.")),
    }

    let interval_s = number(m, "interval").unwrap_or(60);
    let mut channels: Vec<String> = m
        .get("notificationIDList")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, on)| on.as_bool() == Some(true) || on.as_i64() == Some(1))
        .filter_map(|(id, _)| channel_names.get(&id.parse().ok()?).cloned())
        .collect();
    channels.sort();
    let mut monitor = MonitorExport {
        name: text(m, "name").unwrap_or_default(),
        group: number(m, "parent").and_then(|p| groups.get(&p).cloned()),
        interval_s,
        // Kuma retries `maxretries` times before Down: one more failure in all.
        failure_threshold: (number(m, "maxretries").unwrap_or(0) + 1).clamp(1, 10),
        paused: flag(m, "active") == Some(false),
        channels,
        ..MonitorExport::default()
    };

    if kind == "push" {
        monitor.kind = "push".to_string();
        monitor.tls_expiry_warn_days = None;
        // Kuma's token is shorter than this application's: the import gives a new one.
        monitor.push_token = text(m, "pushToken");
        return Ok(Some(monitor));
    }

    monitor.url = text(m, "url").unwrap_or_default();
    monitor.method = text(m, "method").unwrap_or_else(|| "GET".into());
    monitor.headers = text(m, "headers");
    monitor.body = text(m, "body");
    if kind == "keyword" {
        monitor.keyword = text(m, "keyword");
    }
    monitor.retry_interval_s = number(m, "retryInterval")
        .unwrap_or(interval_s)
        .min(interval_s);
    // Kuma's default timeout is 80 % of the interval.
    monitor.timeout_s = number(m, "timeout")
        .filter(|t| *t > 0)
        .unwrap_or(interval_s * 4 / 5)
        .clamp(1, 120)
        .min(interval_s);
    let statuses: Vec<String> = m
        .get("accepted_statuscodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect();
    if !statuses.is_empty() {
        monitor.accepted_statuses = statuses.join(",");
    }
    monitor.follow_redirects = number(m, "maxredirects").unwrap_or(10) > 0;
    monitor.ignore_tls_errors = flag(m, "ignoreTls").unwrap_or(false);
    monitor.tls_expiry_warn_days = flag(m, "expiryNotification")
        .unwrap_or(false)
        .then_some(TLS_WARN_DAYS);
    Ok(Some(monitor))
}
