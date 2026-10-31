use crate::forms::monitor::FieldErrors;
use crate::forms::{has_visible_char, text_len};
use crate::models::name_key;
use crate::models::{Maintenance, MaintenanceInput};
use crate::windows::{MAX_WEEKLY_MINUTES, Schedule};
use jiff::civil::{DateTime, Time};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use serde::Deserialize;
use std::collections::BTreeMap;

const TITLE_MAX_CHARS: usize = 100;
const DAY_PREFIX: &str = "day_";
const MONITOR_PREFIX: &str = "monitor_";
const GROUP_PREFIX: &str = "group_";
/// `datetime-local` inputs, in the instance timezone.
const DATETIME_FORMAT: &str = "%Y-%m-%dT%H:%M";

/// Raw maintenance form, as submitted.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MaintenanceForm {
    pub title: String,
    /// `once` or `weekly`.
    pub kind: String,
    pub starts_at: String,
    pub ends_at: String,
    /// `HH:MM`.
    pub local_start: String,
    pub duration_min: String,
    pub all_monitors: Option<String>,
    pub show_on_status_pages: Option<String>,
    /// Every other field: `day_0` (Monday) to `day_6`, `monitor_<id>`, and `group_<key>` which
    /// picks the monitors of a group when the form is saved.
    #[serde(flatten)]
    pub other: BTreeMap<String, String>,
}

impl MaintenanceForm {
    pub fn new_defaults() -> Self {
        Self {
            kind: "once".to_string(),
            duration_min: "60".to_string(),
            ..Self::default()
        }
    }

    /// The form of a stored maintenance, its times in `tz`.
    pub fn from_maintenance(m: &Maintenance, monitor_ids: &[i64], tz: &TimeZone) -> Self {
        let mut form = Self {
            title: m.title.clone(),
            all_monitors: m.all_monitors.then(|| "on".to_string()),
            show_on_status_pages: m.show_on_status_pages.then(|| "on".to_string()),
            ..Self::new_defaults()
        };
        match m.schedule() {
            Schedule::Once { starts_at, ends_at } => {
                form.starts_at = format_local(tz, starts_at);
                form.ends_at = format_local(tz, ends_at);
            }
            Schedule::Weekly {
                weekdays,
                local_start,
                duration_min,
            } => {
                form.kind = "weekly".to_string();
                for day in 0..7 {
                    if weekdays & (1 << day) != 0 {
                        form.other
                            .insert(format!("{DAY_PREFIX}{day}"), "on".to_string());
                    }
                }
                form.local_start = format!("{:02}:{:02}", local_start / 60, local_start % 60);
                form.duration_min = duration_min.to_string();
            }
        }
        for id in monitor_ids {
            form.other
                .insert(format!("{MONITOR_PREFIX}{id}"), "on".to_string());
        }
        form
    }

    pub fn is_weekly(&self) -> bool {
        self.kind == "weekly"
    }

    pub fn has_day(&self, day: usize) -> bool {
        self.other.contains_key(&format!("{DAY_PREFIX}{day}"))
    }

    pub fn includes(&self, monitor_id: i64) -> bool {
        self.other
            .contains_key(&format!("{MONITOR_PREFIX}{monitor_id}"))
    }

    pub fn has_group(&self, group: &str) -> bool {
        self.other.contains_key(&group_field(group))
    }

    /// `monitors` lists every monitor's id and group; times are read in `tz`.
    pub fn validate(
        &self,
        tz: &TimeZone,
        monitors: &[(i64, Option<String>)],
    ) -> Result<MaintenanceInput, FieldErrors> {
        let mut errors = FieldErrors::new();

        let title = self.title.trim();
        if !has_visible_char(title) {
            errors.insert("title", "Title is required.".to_string());
        } else if text_len(title) > TITLE_MAX_CHARS {
            errors.insert(
                "title",
                format!("Title must be at most {TITLE_MAX_CHARS} characters."),
            );
        }

        let schedule = if self.is_weekly() {
            self.weekly(&mut errors)
        } else {
            self.once(tz, &mut errors)
        };

        let monitor_ids: Vec<i64> = monitors
            .iter()
            .filter(|(id, group)| {
                self.includes(*id) || group.as_deref().is_some_and(|g| self.has_group(g))
            })
            .map(|(id, _)| *id)
            .collect();
        let all_monitors = self.all_monitors.is_some();
        if !all_monitors && monitor_ids.is_empty() {
            errors.insert("monitors", "Pick at least one monitor.".to_string());
        }

        match schedule {
            Some(schedule) if errors.is_empty() => Ok(MaintenanceInput {
                title: title.to_string(),
                schedule,
                all_monitors,
                show_on_status_pages: self.show_on_status_pages.is_some(),
                monitor_ids,
            }),
            _ => Err(errors),
        }
    }

    fn once(&self, tz: &TimeZone, errors: &mut FieldErrors) -> Option<Schedule> {
        let starts_at = parse_local(tz, &self.starts_at);
        let ends_at = parse_local(tz, &self.ends_at);
        if starts_at.is_none() {
            errors.insert("starts_at", "Enter a start date and time.".to_string());
        }
        if ends_at.is_none() {
            errors.insert("ends_at", "Enter an end date and time.".to_string());
        }
        let (starts_at, ends_at) = (starts_at?, ends_at?);
        if ends_at <= starts_at {
            errors.insert("ends_at", "The end must come after the start.".to_string());
            return None;
        }
        Some(Schedule::Once { starts_at, ends_at })
    }

    fn weekly(&self, errors: &mut FieldErrors) -> Option<Schedule> {
        let weekdays = (0..7u8)
            .filter(|day| self.has_day(usize::from(*day)))
            .fold(0u8, |mask, day| mask | 1 << day);
        if weekdays == 0 {
            errors.insert("days", "Pick at least one day.".to_string());
        }
        let local_start = self
            .local_start
            .trim()
            .parse::<Time>()
            .ok()
            .map(|t| i64::from(t.hour()) * 60 + i64::from(t.minute()));
        if local_start.is_none() {
            errors.insert("local_start", "Enter a time such as 02:30.".to_string());
        }
        let duration_min = self.duration_min.trim().parse::<i64>().ok();
        let longest = longest_duration(weekdays);
        let duration_min = match duration_min {
            Some(d) if !(1..=MAX_WEEKLY_MINUTES).contains(&d) => {
                errors.insert(
                    "duration_min",
                    format!("Duration must be 1 to {MAX_WEEKLY_MINUTES} minutes."),
                );
                None
            }
            Some(d) if d > longest => {
                errors.insert(
                    "duration_min",
                    format!(
                        "Duration must be at most {longest} minutes, the time between two of its \
                         days: occurrences may not overlap."
                    ),
                );
                None
            }
            Some(d) => Some(d),
            None => {
                errors.insert(
                    "duration_min",
                    format!("Duration must be 1 to {MAX_WEEKLY_MINUTES} minutes."),
                );
                None
            }
        };
        (weekdays != 0).then_some(Schedule::Weekly {
            weekdays,
            local_start: local_start?,
            duration_min: duration_min?,
        })
    }
}

/// The longest duration whose occurrences on `weekdays` never overlap: the shortest gap between
/// two picked days, a week for a single day.
fn longest_duration(weekdays: u8) -> i64 {
    let days: Vec<i64> = (0..7).filter(|d| weekdays & (1 << d) != 0).collect();
    let gap = days
        .iter()
        .zip(days.iter().cycle().skip(1))
        .map(|(a, b)| (b - a).rem_euclid(7))
        .map(|g| if g == 0 { 7 } else { g })
        .min()
        .unwrap_or(7);
    gap * 24 * 60
}

/// The checkbox of a group: its name compared as group names are.
pub fn group_field(group: &str) -> String {
    format!("{GROUP_PREFIX}{}", name_key(group))
}

fn parse_local(tz: &TimeZone, value: &str) -> Option<i64> {
    let local: DateTime = value.trim().parse().ok()?;
    Some(
        local
            .to_zoned(tz.clone())
            .ok()?
            .timestamp()
            .as_millisecond(),
    )
}

fn format_local(tz: &TimeZone, ms: i64) -> String {
    Timestamp::from_millisecond(ms)
        .map(|t| {
            Zoned::new(t, tz.clone())
                .strftime(DATETIME_FORMAT)
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_times_round_trip_in_the_timezone() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let at = parse_local(&paris, "2026-07-01T14:00").unwrap();
        assert_eq!(
            at,
            "2026-07-01T12:00:00Z"
                .parse::<Timestamp>()
                .unwrap()
                .as_millisecond()
        );
        assert_eq!(format_local(&paris, at), "2026-07-01T14:00");
        assert_eq!(parse_local(&paris, "tomorrow"), None);
    }

    #[test]
    fn occurrences_may_last_until_the_next_day_picked() {
        assert_eq!(longest_duration(0b1), MAX_WEEKLY_MINUTES);
        assert_eq!(longest_duration(0b11), 1440);
        // Monday and Thursday: 3 days apart one way, 4 the other.
        assert_eq!(longest_duration(0b1001), 3 * 1440);
        assert_eq!(longest_duration(0b100_0001), 1440, "Sunday to Monday");
    }

    #[test]
    fn stored_weekly_window_fills_the_form() {
        let form = MaintenanceForm {
            title: "Backups".to_string(),
            kind: "weekly".to_string(),
            local_start: "02:05".to_string(),
            duration_min: "30".to_string(),
            other: BTreeMap::from([
                ("day_2".to_string(), "on".to_string()),
                ("monitor_7".to_string(), "on".to_string()),
            ]),
            ..MaintenanceForm::default()
        };
        let input = form
            .validate(&TimeZone::UTC, &[(7, None), (8, None)])
            .unwrap();
        assert_eq!(
            input.schedule,
            Schedule::Weekly {
                weekdays: 0b100,
                local_start: 125,
                duration_min: 30
            }
        );
        assert_eq!(input.monitor_ids, vec![7]);
    }
}
