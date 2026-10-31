use crate::error::AppError;
use crate::forms::maintenance::{MaintenanceForm, group_field};
use crate::forms::monitor::FieldErrors;
use crate::models::{Maintenance, Monitor, MonitorSummary, Settings};
use crate::time::{day_of, format_datetime, format_duration, format_time, now_ms};
use crate::windows::Schedule;
use askama::Template;
use askama_web::WebTemplate;
use jiff::tz::TimeZone;
use sqlx::SqlitePool;
use std::collections::HashMap;

pub const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
/// Monitors named in a row of the list; beyond, a count.
const NAMED_MONITORS: usize = 3;

#[derive(Template, WebTemplate)]
#[template(path = "maintenances/index.html")]
pub struct IndexTemplate {
    pub current: Vec<MaintenanceRow>,
    pub upcoming: Vec<MaintenanceRow>,
    pub past: Vec<MaintenanceRow>,
    pub csrf_token: String,
}

pub struct MaintenanceRow {
    pub id: i64,
    pub title: String,
    /// `2030-01-02 10:00 → 12:30` or `Mon, Sun at 02:30 for 1 h 30 min`.
    pub schedule: String,
    /// `Until 12:30`, `Next on 2030-01-02 10:00`, `Ended 2030-01-02 12:30`.
    pub status: String,
    pub monitors: String,
    pub public: bool,
}

impl IndexTemplate {
    pub async fn load(pool: &SqlitePool, csrf_token: String) -> Result<Self, AppError> {
        let tz = Settings::load(pool).await?.tz();
        let now = now_ms();
        let names: HashMap<i64, String> = Monitor::summaries(pool)
            .await?
            .into_iter()
            .map(|m| (m.id, m.name))
            .collect();
        let mut page = Self {
            current: Vec::new(),
            upcoming: Vec::new(),
            past: Vec::new(),
            csrf_token,
        };
        let mut sorted = Vec::new();
        for (m, ids) in Maintenance::all(pool).await? {
            let schedule = m.schedule();
            let (list, key, status) = if let Some((_, end)) = schedule.occurrence_at(&tz, now) {
                (0, end, format!("Until {}", short_time(&tz, end, now)))
            } else if let Some((start, _)) = schedule.next_after(&tz, now) {
                (1, start, format!("Next on {}", format_datetime(&tz, start)))
            } else {
                let Schedule::Once { ends_at, .. } = schedule else {
                    continue;
                };
                // Latest first.
                (
                    2,
                    -ends_at,
                    format!("Ended {}", format_datetime(&tz, ends_at)),
                )
            };
            let row = MaintenanceRow {
                id: m.id,
                title: m.title,
                schedule: describe(&schedule, &tz),
                status,
                monitors: monitors_label(m.all_monitors, &ids, &names),
                public: m.show_on_status_pages,
            };
            sorted.push((list, key, row));
        }
        sorted.sort_by_key(|(list, key, _)| (*list, *key));
        for (list, _, row) in sorted {
            match list {
                0 => page.current.push(row),
                1 => page.upcoming.push(row),
                _ => page.past.push(row),
            }
        }
        Ok(page)
    }
}

/// When a window is on, in words.
pub fn describe(schedule: &Schedule, tz: &TimeZone) -> String {
    match *schedule {
        Schedule::Once { starts_at, ends_at } => {
            let end = if day_of(tz, starts_at) == day_of(tz, ends_at) {
                format_time(tz, ends_at)
            } else {
                format_datetime(tz, ends_at)
            };
            format!("{} → {end}", format_datetime(tz, starts_at))
        }
        Schedule::Weekly {
            weekdays,
            local_start,
            duration_min,
        } => {
            let days: Vec<&str> = WEEKDAYS
                .iter()
                .enumerate()
                .filter(|(i, _)| weekdays & (1 << i) != 0)
                .map(|(_, d)| *d)
                .collect();
            let days = if days.len() == 7 {
                "Every day".to_string()
            } else {
                days.join(", ")
            };
            format!(
                "{days} at {:02}:{:02} for {}",
                local_start / 60,
                local_start % 60,
                format_duration(duration_min * 60_000)
            )
        }
    }
}

/// `14:30` today, the full date otherwise.
pub fn short_time(tz: &TimeZone, at: i64, now: i64) -> String {
    if day_of(tz, at) == day_of(tz, now) {
        format_time(tz, at)
    } else {
        format_datetime(tz, at)
    }
}

fn monitors_label(all: bool, ids: &[i64], names: &HashMap<i64, String>) -> String {
    if all {
        return "All monitors".to_string();
    }
    let mut named: Vec<&str> = ids
        .iter()
        .filter_map(|id| names.get(id).map(String::as_str))
        .collect();
    named.sort_unstable_by_key(|n| n.to_lowercase());
    match named.len() {
        0 => "No monitor".to_string(),
        n if n <= NAMED_MONITORS => named.join(", "),
        n => format!(
            "{} and {} more",
            named[..NAMED_MONITORS - 1].join(", "),
            n - (NAMED_MONITORS - 1)
        ),
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "maintenances/form.html")]
pub struct FormTemplate {
    pub form: MaintenanceForm,
    pub errors: FieldErrors,
    pub csrf_token: String,
    pub action_url: String,
    pub maintenance_id: Option<i64>,
    pub monitors: Vec<MonitorSummary>,
    /// Group names in use, for the group checkboxes.
    pub groups: Vec<String>,
    /// The instance timezone, which the times are in.
    pub timezone: String,
}

impl FormTemplate {
    pub fn is_edit(&self) -> bool {
        self.maintenance_id.is_some()
    }

    pub fn error(&self, field: &str) -> Option<&str> {
        self.errors.get(field).map(String::as_str)
    }

    pub fn weekdays(&self) -> &'static [&'static str] {
        &WEEKDAYS
    }

    pub fn group_field(&self, group: &str) -> String {
        group_field(group)
    }
}
