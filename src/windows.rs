//! Maintenance windows in time: when a window is on, and when it next starts. Pure: the caller
//! gives the instant and the instance timezone.

use crate::models::{Maintenance, Settings};
use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use sqlx::SqlitePool;
use std::sync::{Arc, RwLock};

const MINUTE_MS: i64 = 60_000;
/// A weekly window lasts at most a week, so it never overlaps its next occurrence.
pub const MAX_WEEKLY_MINUTES: i64 = 7 * 24 * 60;

/// When a maintenance window is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// From `starts_at` (included) to `ends_at` (excluded), in ms.
    Once { starts_at: i64, ends_at: i64 },
    /// On the days of `weekdays` (bit 0 = Monday … bit 6 = Sunday), from `local_start` minutes
    /// after local midnight, for `duration_min` minutes of real time.
    Weekly {
        weekdays: u8,
        local_start: i64,
        duration_min: i64,
    },
}

impl Schedule {
    /// The occurrence holding `at`, as `(start, end)` in ms.
    pub fn occurrence_at(&self, tz: &TimeZone, at: i64) -> Option<(i64, i64)> {
        match *self {
            Self::Once { starts_at, ends_at } => {
                (starts_at <= at && at < ends_at).then_some((starts_at, ends_at))
            }
            Self::Weekly { duration_min, .. } => {
                let today = local_date(tz, at)?;
                // An occurrence started at most `duration_min` ago, so on one of these days.
                let days_back = duration_min.div_euclid(24 * 60) + 1;
                (0..=days_back)
                    .filter_map(|back| today.checked_sub(back.days()).ok())
                    .filter_map(|day| self.occurrence_on(tz, day))
                    .find(|&(start, end)| start <= at && at < end)
            }
        }
    }

    /// The first occurrence starting after `at` (excluded), as `(start, end)`.
    pub fn next_after(&self, tz: &TimeZone, at: i64) -> Option<(i64, i64)> {
        match *self {
            Self::Once { starts_at, ends_at } => (starts_at > at).then_some((starts_at, ends_at)),
            Self::Weekly { .. } => {
                let today = local_date(tz, at)?;
                (0..=7)
                    .filter_map(|ahead| today.checked_add(ahead.days()).ok())
                    .filter_map(|day| self.occurrence_on(tz, day))
                    .find(|&(start, _)| start > at)
            }
        }
    }

    /// Occurrences overlapping `[from, to)`, earliest first.
    pub fn occurrences_between(&self, tz: &TimeZone, from: i64, to: i64) -> Vec<(i64, i64)> {
        match *self {
            Self::Once { starts_at, ends_at } => {
                if starts_at < to && from < ends_at {
                    vec![(starts_at, ends_at)]
                } else {
                    vec![]
                }
            }
            Self::Weekly { duration_min, .. } => {
                let (Some(first), Some(last)) = (local_date(tz, from), local_date(tz, to)) else {
                    return vec![];
                };
                let mut day = first
                    .checked_sub((duration_min.div_euclid(24 * 60) + 1).days())
                    .unwrap_or(first);
                let mut found = Vec::new();
                while day <= last {
                    if let Some((start, end)) = self.occurrence_on(tz, day)
                        && start < to
                        && from < end
                    {
                        found.push((start, end));
                    }
                    let Ok(next) = day.tomorrow() else { break };
                    day = next;
                }
                found
            }
        }
    }

    /// The occurrence of a weekly window starting on local date `day`, if `day` is one of its
    /// days. A start in a DST gap moves forward by the gap; in a fold, the first one is taken.
    fn occurrence_on(&self, tz: &TimeZone, day: Date) -> Option<(i64, i64)> {
        let Self::Weekly {
            weekdays,
            local_start,
            duration_min,
        } = *self
        else {
            return None;
        };
        let bit = day.weekday().to_monday_zero_offset();
        if weekdays & (1 << bit) == 0 {
            return None;
        }
        let time = Time::new(
            i8::try_from(local_start / 60).ok()?,
            i8::try_from(local_start % 60).ok()?,
            0,
            0,
        )
        .ok()?;
        let start = day.to_datetime(time).to_zoned(tz.clone()).ok()?;
        let start = start.timestamp().as_millisecond();
        Some((start, start + duration_min * MINUTE_MS))
    }
}

fn local_date(tz: &TimeZone, at: i64) -> Option<Date> {
    Some(
        Timestamp::from_millisecond(at)
            .ok()?
            .to_zoned(tz.clone())
            .date(),
    )
}

/// The maintenance windows, kept in memory and shared by the engine and the pages. Few and
/// small: reloaded whole after any change to a window or to the timezone.
#[derive(Clone, Default)]
pub struct Maintenances {
    loaded: Arc<RwLock<Loaded>>,
    /// One reload at a time, so an older read never overwrites a newer one.
    reloading: Arc<tokio::sync::Mutex<()>>,
}

struct Loaded {
    tz: TimeZone,
    windows: Vec<Window>,
}

impl Default for Loaded {
    fn default() -> Self {
        Self {
            tz: TimeZone::UTC,
            windows: Vec::new(),
        }
    }
}

struct Window {
    title: String,
    schedule: Schedule,
    show_on_status_pages: bool,
    all_monitors: bool,
    monitor_ids: Vec<i64>,
}

impl Window {
    fn applies_to(&self, monitor_id: i64) -> bool {
        self.all_monitors || self.monitor_ids.contains(&monitor_id)
    }
}

/// A window as a status page shows it. The title is left out unless the window is announced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: Option<String>,
    pub starts_at: i64,
    pub ends_at: i64,
}

/// A window on now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Current {
    pub title: String,
    pub ends_at: i64,
}

impl Maintenances {
    pub async fn reload(&self, pool: &SqlitePool) -> Result<(), sqlx::Error> {
        let _turn = self.reloading.lock().await;
        let tz = Settings::load(pool).await?.tz();
        let windows = Maintenance::all(pool)
            .await?
            .into_iter()
            .map(|(m, monitor_ids)| Window {
                title: m.title.clone(),
                schedule: m.schedule(),
                show_on_status_pages: m.show_on_status_pages,
                all_monitors: m.all_monitors,
                monitor_ids,
            })
            .collect();
        *self.loaded.write().unwrap() = Loaded { tz, windows };
        Ok(())
    }

    /// Whether a window covers `monitor_id` at `at`.
    pub fn covers(&self, monitor_id: i64, at: i64) -> bool {
        self.current(monitor_id, at).is_some()
    }

    /// The window covering `monitor_id` at `at` that ends last, if any.
    pub fn current(&self, monitor_id: i64, at: i64) -> Option<Current> {
        let loaded = self.loaded.read().unwrap();
        loaded
            .windows
            .iter()
            .filter(|w| w.applies_to(monitor_id))
            .filter_map(|w| {
                let (_, ends_at) = w.schedule.occurrence_at(&loaded.tz, at)?;
                Some(Current {
                    title: w.title.clone(),
                    ends_at,
                })
            })
            .max_by_key(|c| c.ends_at)
    }

    /// For a status page showing `monitor_ids`: the windows on at `now`, then the announced
    /// ones starting within `horizon_ms`, each by start.
    pub fn notices(
        &self,
        monitor_ids: &[i64],
        now: i64,
        horizon_ms: i64,
    ) -> (Vec<Notice>, Vec<Notice>) {
        let loaded = self.loaded.read().unwrap();
        let (mut current, mut planned) = (Vec::new(), Vec::new());
        for w in &loaded.windows {
            if !monitor_ids.iter().any(|&id| w.applies_to(id)) {
                continue;
            }
            let notice = |(starts_at, ends_at)| Notice {
                title: w.show_on_status_pages.then(|| w.title.clone()),
                starts_at,
                ends_at,
            };
            if let Some(on) = w.schedule.occurrence_at(&loaded.tz, now) {
                current.push(notice(on));
            } else if w.show_on_status_pages
                && let Some(next) = w.schedule.next_after(&loaded.tz, now)
                && next.0 <= now + horizon_ms
            {
                planned.push(notice(next));
            }
        }
        current.sort_by_key(|n| n.starts_at);
        planned.sort_by_key(|n| n.starts_at);
        (current, planned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;
    /// Monday to Friday.
    const WORKDAYS: u8 = 0b001_1111;

    fn ms(s: &str) -> i64 {
        s.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn weekly(weekdays: u8, local_start: i64, duration_min: i64) -> Schedule {
        Schedule::Weekly {
            weekdays,
            local_start,
            duration_min,
        }
    }

    #[test]
    fn a_one_off_window_includes_its_start_not_its_end() {
        let once = Schedule::Once {
            starts_at: 100,
            ends_at: 200,
        };
        assert_eq!(once.occurrence_at(&TimeZone::UTC, 99), None);
        assert_eq!(once.occurrence_at(&TimeZone::UTC, 100), Some((100, 200)));
        assert_eq!(once.occurrence_at(&TimeZone::UTC, 199), Some((100, 200)));
        assert_eq!(once.occurrence_at(&TimeZone::UTC, 200), None);
        assert_eq!(once.next_after(&TimeZone::UTC, 50), Some((100, 200)));
        assert_eq!(once.next_after(&TimeZone::UTC, 100), None);
    }

    #[test]
    fn a_weekly_window_follows_local_time_on_its_days() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        // 02:00 to 03:30 local, Monday to Friday. 2026-10-28 is a Wednesday (UTC+1).
        let w = weekly(WORKDAYS, 120, 90);
        let start = ms("2026-10-28T01:00:00Z");
        assert_eq!(
            w.occurrence_at(&paris, start + HOUR),
            Some((start, start + 90 * 60_000))
        );
        assert_eq!(w.occurrence_at(&paris, start - 1), None);
        assert_eq!(w.occurrence_at(&paris, start + 90 * 60_000), None);
        // Saturday 2026-10-31 is not one of its days.
        assert_eq!(w.occurrence_at(&paris, ms("2026-10-31T01:30:00Z")), None);
        // From Friday after the window, the next one is on Monday.
        assert_eq!(
            w.next_after(&paris, ms("2026-10-30T12:00:00Z")),
            Some((
                ms("2026-11-02T01:00:00Z"),
                ms("2026-11-02T01:00:00Z") + 90 * 60_000
            ))
        );
    }

    #[test]
    fn a_window_crossing_midnight_is_found_the_next_day() {
        // Sunday 23:00 UTC for 3 hours: still on at Monday 01:00.
        let w = weekly(1 << 6, 23 * 60, 180);
        let start = ms("2026-11-01T23:00:00Z");
        assert_eq!(
            w.occurrence_at(&TimeZone::UTC, ms("2026-11-02T01:00:00Z")),
            Some((start, start + 3 * HOUR))
        );
    }

    #[test]
    fn a_weekly_window_keeps_its_local_hour_across_dst() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let w = weekly(0b111_1111, 12 * 60, 60);
        // Noon in Paris: 11:00Z in winter (UTC+1), 10:00Z in summer (UTC+2).
        assert!(
            w.occurrence_at(&paris, ms("2026-12-01T11:30:00Z"))
                .is_some()
        );
        assert!(
            w.occurrence_at(&paris, ms("2026-07-01T10:30:00Z"))
                .is_some()
        );
        assert!(
            w.occurrence_at(&paris, ms("2026-07-01T11:30:00Z"))
                .is_none()
        );
        // 02:30 does not exist on 2026-03-29 in Paris: the window starts an hour later, 03:30.
        let gap = weekly(1 << 6, 150, 30);
        assert_eq!(
            gap.occurrence_at(&paris, ms("2026-03-29T01:40:00Z")),
            Some((ms("2026-03-29T01:30:00Z"), ms("2026-03-29T02:00:00Z")))
        );
    }

    #[test]
    fn occurrences_between_lists_those_overlapping_the_range() {
        let w = weekly(WORKDAYS, 0, 60);
        let found = w.occurrences_between(
            &TimeZone::UTC,
            ms("2026-10-30T00:30:00Z"),
            ms("2026-11-03T00:00:00Z"),
        );
        assert_eq!(
            found,
            vec![
                (ms("2026-10-30T00:00:00Z"), ms("2026-10-30T01:00:00Z")),
                (ms("2026-11-02T00:00:00Z"), ms("2026-11-02T01:00:00Z")),
            ]
        );
    }
}
