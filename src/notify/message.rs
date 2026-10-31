//! What a notification says, whatever the channel. Pure: the clock and timezone are passed in.

use crate::notify::{Event, MonitorRef, Notification};
use crate::time::{day_of, days_left, format_datetime, format_duration};
use jiff::tz::TimeZone;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Critical,
    Resolved,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub text: String,
    pub severity: Severity,
    /// The monitor's detail page, when `PUBLIC_URL` is set.
    pub link: Option<String>,
}

impl Message {
    pub fn new(notification: &Notification, tz: &TimeZone, public_url: Option<&str>) -> Self {
        let name = notification
            .monitor
            .as_ref()
            .map_or("", |m| m.name.as_str());
        let (title, text, severity) = match &notification.event {
            Event::Down { since, cause } => (
                format!("{name} is down"),
                format!("{cause}\nDown since {}.", format_datetime(tz, *since)),
                Severity::Critical,
            ),
            Event::Up { down_since } => (
                format!("{name} is up"),
                format!(
                    "Back up after {} of downtime.",
                    format_duration(notification.at - down_since)
                ),
                Severity::Resolved,
            ),
            Event::TlsExpiring { expires_at } => {
                let date = day_of(tz, *expires_at);
                match days_left(*expires_at, notification.at) {
                    left if left < 0 => (
                        format!("{name}: certificate expired"),
                        format!("The certificate expired on {date}."),
                        Severity::Warning,
                    ),
                    left => (
                        format!("{name}: certificate expires soon"),
                        format!(
                            "The certificate expires on {date} ({left} day{} left).",
                            if left == 1 { "" } else { "s" }
                        ),
                        Severity::Warning,
                    ),
                }
            }
            Event::Degraded {
                since,
                latency_ms,
                threshold_ms,
            } => (
                format!("{name} is slow"),
                format!(
                    "Responses take {latency_ms} ms, above the {threshold_ms} ms threshold.\nSlow since {}.",
                    format_datetime(tz, *since)
                ),
                Severity::Warning,
            ),
            Event::LatencyNormal { degraded_since } => (
                format!("{name} is fast again"),
                format!(
                    "Response times are back to normal after {}.",
                    format_duration(notification.at - degraded_since)
                ),
                Severity::Resolved,
            ),
            Event::Test => (
                "Test notification".to_string(),
                "This channel works.".to_string(),
                Severity::Info,
            ),
        };
        let text = match &notification.monitor {
            Some(MonitorRef { url: Some(url), .. }) => format!("{text}\n{url}"),
            _ => text,
        };
        let link = public_url
            .zip(notification.monitor.as_ref())
            .map(|(base, monitor)| format!("{base}/monitors/{}", monitor.id));
        Self {
            title,
            text,
            severity,
            link,
        }
    }

    /// One message for several monitors going down, or several coming back up, at once: a line
    /// per monitor, at most `GROUP_MAX_LINES`, each cut to `GROUP_LINE_MAX_CHARS` so that the
    /// message fits Discord's and ntfy's 4,096 characters. Links to the dashboard.
    pub fn group(notifications: &[&Notification], public_url: Option<&str>) -> Self {
        let up = matches!(
            notifications.first().map(|n| &n.event),
            Some(Event::Up { .. })
        );
        let mut lines: Vec<String> = notifications
            .iter()
            .take(GROUP_MAX_LINES)
            .map(|n| {
                let name = n.monitor.as_ref().map_or("", |m| m.name.as_str());
                let detail = match &n.event {
                    Event::Down { cause, .. } => cause.clone(),
                    Event::Up { down_since } => {
                        format!("back up after {}", format_duration(n.at - down_since))
                    }
                    other => other.as_str().to_string(),
                };
                cut(&format!("{name}: {detail}"), GROUP_LINE_MAX_CHARS)
            })
            .collect();
        if notifications.len() > GROUP_MAX_LINES {
            lines.push(format!(
                "…and {} more.",
                notifications.len() - GROUP_MAX_LINES
            ));
        }
        let (state, severity) = if up {
            ("up", Severity::Resolved)
        } else {
            ("down", Severity::Critical)
        };
        Self {
            title: format!("{} monitors are {state}", notifications.len()),
            text: lines.join("\n"),
            severity,
            link: public_url.map(|base| format!("{base}/")),
        }
    }
}

const GROUP_MAX_LINES: usize = 20;
const GROUP_LINE_MAX_CHARS: usize = 150;

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max - 1).chain(['…']).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::MonitorRef;
    use crate::time::DAY_MS;

    fn notification(event: Event) -> Notification {
        Notification {
            monitor: Some(MonitorRef {
                id: 1,
                name: "API".to_string(),
                url: Some("https://api.example.com".to_string()),
            }),
            event,
            at: 10 * DAY_MS,
        }
    }

    fn message(event: Event) -> Message {
        Message::new(&notification(event), &TimeZone::UTC, None)
    }

    #[test]
    fn link_points_to_the_detail_page_when_public_url_is_set() {
        let up = notification(Event::Up { down_since: 0 });
        assert_eq!(message(Event::Up { down_since: 0 }).link, None);
        assert_eq!(
            Message::new(&up, &TimeZone::UTC, Some("https://status.example.com")).link,
            Some("https://status.example.com/monitors/1".to_string())
        );
        let test = Notification {
            monitor: None,
            event: Event::Test,
            at: 0,
        };
        assert_eq!(
            Message::new(&test, &TimeZone::UTC, Some("https://status.example.com")).link,
            None
        );
    }

    #[test]
    fn push_monitor_text_has_no_url_line() {
        let mut down = notification(Event::Down {
            since: 10 * DAY_MS,
            cause: "no ping for 120 s".to_string(),
        });
        down.monitor.as_mut().unwrap().url = None;
        let m = Message::new(&down, &TimeZone::UTC, None);
        assert_eq!(m.text, "no ping for 120 s\nDown since 1970-01-11 00:00.");
    }

    #[test]
    fn down_gives_the_cause_and_start() {
        let m = message(Event::Down {
            since: 10 * DAY_MS - 90_000,
            cause: "HTTP 503, expected 200-299".to_string(),
        });
        assert_eq!(m.title, "API is down");
        assert_eq!(
            m.text,
            "HTTP 503, expected 200-299\nDown since 1970-01-10 23:58.\nhttps://api.example.com"
        );
        assert_eq!(m.severity, Severity::Critical);
    }

    #[test]
    fn up_gives_the_downtime() {
        let m = message(Event::Up {
            down_since: 10 * DAY_MS - 3_900_000,
        });
        assert_eq!(m.title, "API is up");
        assert_eq!(
            m.text,
            "Back up after 1 h 05 min of downtime.\nhttps://api.example.com"
        );
        assert_eq!(m.severity, Severity::Resolved);
    }

    #[test]
    fn certificate_warning_counts_the_days_left() {
        let m = message(Event::TlsExpiring {
            expires_at: 19 * DAY_MS + 3_600_000,
        });
        assert_eq!(m.title, "API: certificate expires soon");
        assert_eq!(
            m.text,
            "The certificate expires on 1970-01-20 (9 days left).\nhttps://api.example.com"
        );
        assert_eq!(m.severity, Severity::Warning);

        let one = message(Event::TlsExpiring {
            expires_at: 11 * DAY_MS,
        });
        assert!(one.text.contains("(1 day left)"), "{}", one.text);

        let expired = message(Event::TlsExpiring {
            expires_at: 9 * DAY_MS,
        });
        assert_eq!(expired.title, "API: certificate expired");
        assert!(
            expired
                .text
                .starts_with("The certificate expired on 1970-01-10."),
            "{}",
            expired.text
        );
    }

    #[test]
    fn degraded_gives_the_latency_and_threshold() {
        let m = message(Event::Degraded {
            since: 10 * DAY_MS - 180_000,
            latency_ms: 1_240,
            threshold_ms: 800,
        });
        assert_eq!(m.title, "API is slow");
        assert_eq!(
            m.text,
            "Responses take 1240 ms, above the 800 ms threshold.\nSlow since 1970-01-10 23:57.\nhttps://api.example.com"
        );
        assert_eq!(m.severity, Severity::Warning);
    }

    #[test]
    fn latency_normal_gives_the_slowdown_length() {
        let m = message(Event::LatencyNormal {
            degraded_since: 10 * DAY_MS - 600_000,
        });
        assert_eq!(m.title, "API is fast again");
        assert_eq!(
            m.text,
            "Response times are back to normal after 10 min.\nhttps://api.example.com"
        );
        assert_eq!(m.severity, Severity::Resolved);
    }

    fn of(id: i64, name: &str, event: Event) -> Notification {
        Notification {
            monitor: Some(MonitorRef {
                id,
                name: name.to_string(),
                url: None,
            }),
            event,
            at: 10 * DAY_MS,
        }
    }

    #[test]
    fn group_of_downs_lists_each_cause() {
        let a = of(
            1,
            "API",
            Event::Down {
                since: 0,
                cause: "HTTP 503".to_string(),
            },
        );
        let b = of(
            2,
            "Web",
            Event::Down {
                since: 0,
                cause: "timeout".to_string(),
            },
        );
        let m = Message::group(&[&a, &b], Some("https://up.example.com"));
        assert_eq!(m.title, "2 monitors are down");
        assert_eq!(m.text, "API: HTTP 503\nWeb: timeout");
        assert_eq!(m.severity, Severity::Critical);
        assert_eq!(m.link.as_deref(), Some("https://up.example.com/"));
    }

    #[test]
    fn group_of_ups_gives_each_downtime() {
        let a = of(
            1,
            "API",
            Event::Up {
                down_since: 10 * DAY_MS - 120_000,
            },
        );
        let b = of(
            2,
            "Web",
            Event::Up {
                down_since: 10 * DAY_MS - 60_000,
            },
        );
        let m = Message::group(&[&a, &b], None);
        assert_eq!(m.title, "2 monitors are up");
        assert_eq!(m.text, "API: back up after 2 min\nWeb: back up after 1 min");
        assert_eq!(m.severity, Severity::Resolved);
        assert_eq!(m.link, None);
    }

    #[test]
    fn large_group_stays_short() {
        let downs: Vec<_> = (0..25)
            .map(|i| {
                of(
                    i,
                    "API",
                    Event::Down {
                        since: 0,
                        cause: "x".repeat(500),
                    },
                )
            })
            .collect();
        let refs: Vec<_> = downs.iter().collect();
        let m = Message::group(&refs, None);
        assert_eq!(m.title, "25 monitors are down");
        let lines: Vec<_> = m.text.lines().collect();
        assert_eq!(lines.len(), 21);
        assert_eq!(lines[20], "…and 5 more.");
        assert!(m.text.chars().count() < 4_000, "{}", m.text.chars().count());
    }

    #[test]
    fn test_message_has_no_monitor() {
        let m = Message::new(
            &Notification {
                monitor: None,
                event: Event::Test,
                at: 0,
            },
            &TimeZone::UTC,
            None,
        );
        assert_eq!(m.title, "Test notification");
        assert_eq!(m.text, "This channel works.");
        assert_eq!(m.severity, Severity::Info);
    }
}
