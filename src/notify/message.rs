//! What a notification says, whatever the channel. Pure: the clock and timezone are passed in.

use crate::notify::{Event, Notification};
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
            Event::Test => (
                "Test notification".to_string(),
                "This channel works.".to_string(),
                Severity::Info,
            ),
        };
        let text = match &notification.monitor {
            // A push monitor has no URL.
            Some(monitor) if !monitor.url.is_empty() => format!("{text}\n{}", monitor.url),
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
                url: "https://api.example.com".to_string(),
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
        down.monitor.as_mut().unwrap().url = String::new();
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
