//! Availability shown as bars, one per time bucket.

use crate::error::AppError;
use crate::models::stats::uptime;
use crate::models::{Bucket, DailyStat};
use crate::time::{day_from_key, day_key, day_of, day_start, format_time, hour_start};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, ToSpan};
use sqlx::SqlitePool;
use std::collections::HashMap;

const HOUR_MS: i64 = 3600 * 1000;
/// Hourly bars: the current hour and the 23 before it.
pub const HOURS: i64 = 24;

/// Daily bars: today and the 89 days before it.
pub const DAYS: i64 = 90;
/// The daily uptime figure covers the last 30 bars.
const UPTIME_DAYS: i64 = 30;

/// One bar: its colour class and tooltip.
pub struct Tick {
    pub class: &'static str,
    pub title: String,
}

/// Consecutive bars over a window, and the availability over all of it.
pub struct Heartbeat {
    pub ticks: Vec<Tick>,
    pub monitored_ms: i64,
    pub downtime_ms: i64,
    /// Over the checks that got a response.
    pub latency_avg_ms: Option<i64>,
}

impl Heartbeat {
    /// The last 24 local hours, for one monitor or all of them, by monitor id. Monitors without
    /// any check in that window are absent.
    pub async fn hourly(
        pool: &SqlitePool,
        monitor_id: Option<i64>,
        tz: &TimeZone,
        now: i64,
    ) -> Result<HashMap<i64, Self>, AppError> {
        let from = hourly_start(tz, now);
        let mut by_monitor: HashMap<i64, Vec<Bucket>> = HashMap::new();
        for bucket in Bucket::query(pool, monitor_id, from, HOUR_MS, now + 1).await? {
            by_monitor
                .entry(bucket.monitor_id)
                .or_default()
                .push(bucket);
        }
        Ok(by_monitor
            .into_iter()
            .map(|(id, buckets)| (id, Self::hours(&buckets, tz, from)))
            .collect())
    }

    /// The last 24 hours without any check.
    pub fn empty(tz: &TimeZone, now: i64) -> Self {
        Self::hours(&[], tz, hourly_start(tz, now))
    }

    fn hours(buckets: &[Bucket], tz: &TimeZone, from: i64) -> Self {
        Self::build(buckets, HOURS, |i| {
            let start = from + i * HOUR_MS;
            format!(
                "{}–{}",
                format_time(tz, start),
                format_time(tz, start + HOUR_MS)
            )
        })
    }

    /// `count` bars, bucket `i` in bar `i`, each titled by `label(i)`.
    pub fn build(buckets: &[Bucket], count: i64, label: impl Fn(i64) -> String) -> Self {
        let ticks = (0..count)
            .map(|i| match buckets.iter().find(|b| b.index == i) {
                Some(b) => tick(b, label(i)),
                None => Tick {
                    class: "tick-gray",
                    title: format!("{} · No data", label(i)),
                },
            })
            .collect();
        let latencies: i64 = buckets.iter().map(|b| b.latency_count).sum();
        let latency_sum: i64 = buckets
            .iter()
            .filter_map(|b| Some(b.latency_avg_ms? * b.latency_count))
            .sum();
        Self {
            ticks,
            monitored_ms: buckets.iter().map(|b| b.monitored_ms).sum(),
            downtime_ms: buckets.iter().map(|b| b.downtime_ms).sum(),
            latency_avg_ms: (latencies > 0).then(|| latency_sum / latencies),
        }
    }

    pub fn uptime_label(&self) -> String {
        uptime_label(uptime(self.monitored_ms, self.downtime_ms))
    }
}

/// The last 90 local days of one monitor.
pub struct DailyHistory {
    pub bars: Heartbeat,
    pub uptime_30d: String,
}

impl DailyHistory {
    /// Rolled-up days, then the days not rolled up yet, from raw checks.
    pub async fn load(
        pool: &SqlitePool,
        monitor_id: i64,
        tz: &TimeZone,
        now: i64,
    ) -> Result<Self, AppError> {
        let today = day_of(tz, now);
        let first = today
            .checked_sub(Span::new().days(DAYS - 1))
            .unwrap_or(today);
        let index = |day: Date| first.until(day).map_or(0, |s| i64::from(s.get_days()));

        let mut buckets: Vec<Bucket> = DailyStat::for_monitor(pool, monitor_id, day_key(first))
            .await?
            .into_iter()
            .filter_map(|d| {
                let day = day_from_key(d.day)?;
                Some(Bucket {
                    monitor_id,
                    index: index(day),
                    monitored_ms: d.monitored_ms,
                    downtime_ms: d.downtime_ms,
                    checks_total: d.checks_total,
                    checks_failed: d.checks_failed,
                    latency_avg_ms: d.latency_avg_ms,
                    // Only weighs the average latency, which the daily bars do not show.
                    latency_count: d.checks_total,
                })
            })
            .collect();

        let mut day = match DailyStat::last_day(pool).await?.and_then(day_from_key) {
            Some(done) => done.tomorrow().unwrap_or(today).max(first),
            None => first,
        };
        while day <= today {
            let Ok(next) = day.tomorrow() else { break };
            let (from, to) = (day_start(tz, day), day_start(tz, next));
            for mut b in Bucket::query(pool, Some(monitor_id), from, to - from, to).await? {
                b.index = index(day);
                buckets.push(b);
            }
            day = next;
        }

        let recent: Vec<&Bucket> = buckets
            .iter()
            .filter(|b| b.index >= DAYS - UPTIME_DAYS)
            .collect();
        let uptime_30d = uptime_label(uptime(
            recent.iter().map(|b| b.monitored_ms).sum(),
            recent.iter().map(|b| b.downtime_ms).sum(),
        ));
        let bars = Heartbeat::build(&buckets, DAYS, |i| {
            first
                .checked_add(i.days())
                .map_or_else(|_| String::new(), |d| d.to_string())
        });
        Ok(Self { bars, uptime_30d })
    }
}

fn hourly_start(tz: &TimeZone, now: i64) -> i64 {
    hour_start(tz, now) - (HOURS - 1) * HOUR_MS
}

fn tick(b: &Bucket, span: String) -> Tick {
    let class = if b.downtime_ms > 0 {
        "tick-red"
    } else if b.checks_failed > 0 {
        "tick-orange"
    } else {
        "tick-green"
    };
    Tick {
        class,
        title: format!(
            "{span} · {} · {} failed of {} checks",
            uptime_label(b.uptime()),
            b.checks_failed,
            b.checks_total
        ),
    }
}

/// `99.95%`, truncated so that any downtime shows below 100%; `—` without data.
pub fn uptime_label(uptime: Option<f64>) -> String {
    match uptime {
        None => "—".to_string(),
        Some(u) if u >= 1.0 => "100%".to_string(),
        Some(u) => format!("{:.2}%", (u * 10_000.0).floor() / 100.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_never_rounds_up_to_100() {
        assert_eq!(uptime_label(None), "—");
        assert_eq!(uptime_label(Some(1.0)), "100%");
        assert_eq!(uptime_label(Some(0.99999)), "99.99%");
        assert_eq!(uptime_label(Some(0.75)), "75.00%");
        assert_eq!(uptime_label(Some(0.0)), "0.00%");
    }

    fn bucket(index: i64, monitored_ms: i64, downtime_ms: i64, failed: i64) -> Bucket {
        Bucket {
            monitor_id: 1,
            index,
            monitored_ms,
            downtime_ms,
            checks_total: 10,
            checks_failed: failed,
            latency_avg_ms: None,
            latency_count: 0,
        }
    }

    #[test]
    fn bars_show_downtime_then_failures() {
        let tz = TimeZone::UTC;
        let buckets = [
            bucket(0, 600, 0, 0),
            bucket(1, 600, 0, 2),
            bucket(2, 600, 60, 3),
        ];
        let h = Heartbeat::hours(&buckets, &tz, 0);
        let classes: Vec<_> = h.ticks.iter().take(4).map(|t| t.class).collect();
        assert_eq!(
            classes,
            ["tick-green", "tick-orange", "tick-red", "tick-gray"]
        );
        assert_eq!(
            h.ticks[1].title,
            "01:00–02:00 · 100% · 2 failed of 10 checks"
        );
        assert_eq!(h.ticks[3].title, "03:00–04:00 · No data");
        assert_eq!(h.uptime_label(), "96.66%");
    }
}
