//! Time helpers. Instants are Unix epoch milliseconds; days and hours are those of the instance
//! timezone (`settings.timezone`).

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{RoundMode, Timestamp, Unit, Zoned, ZonedRound};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DAY_MS: i64 = 86_400_000;

/// Whole days from `now` until `at`; negative once `at` is past.
pub fn days_left(at: i64, now: i64) -> i64 {
    (at - now).div_euclid(DAY_MS)
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// The named timezone, or UTC when the name is unknown.
pub fn timezone(name: &str) -> TimeZone {
    TimeZone::get(name).unwrap_or(TimeZone::UTC)
}

fn zoned(tz: &TimeZone, ms: i64) -> Zoned {
    Timestamp::from_millisecond(ms)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned(tz.clone())
}

fn to_ms(zoned: &Zoned) -> i64 {
    zoned.timestamp().as_millisecond()
}

/// The local date of an instant.
pub fn day_of(tz: &TimeZone, ms: i64) -> Date {
    zoned(tz, ms).date()
}

/// The first instant of a local date.
pub fn day_start(tz: &TimeZone, day: Date) -> i64 {
    day.to_zoned(tz.clone())
        .and_then(|z| z.start_of_day())
        .map(|z| to_ms(&z))
        .unwrap_or(0)
}

/// The start of the local hour holding an instant.
pub fn hour_start(tz: &TimeZone, ms: i64) -> i64 {
    let round = ZonedRound::new()
        .smallest(Unit::Hour)
        .mode(RoundMode::Trunc);
    let zoned = zoned(tz, ms);
    zoned.round(round).map(|z| to_ms(&z)).unwrap_or(ms)
}

/// A date as stored in `daily_stats.day`: `YYYYMMDD`.
pub fn day_key(day: Date) -> i64 {
    i64::from(day.year()) * 10_000 + i64::from(day.month()) * 100 + i64::from(day.day())
}

pub fn day_from_key(key: i64) -> Option<Date> {
    let year = i16::try_from(key / 10_000).ok()?;
    Date::new(year, (key / 100 % 100) as i8, (key % 100) as i8).ok()
}

/// `2026-10-28 14:05` in the given timezone.
pub fn format_datetime(tz: &TimeZone, ms: i64) -> String {
    zoned(tz, ms).strftime("%Y-%m-%d %H:%M").to_string()
}

/// `14:05` in the given timezone.
pub fn format_time(tz: &TimeZone, ms: i64) -> String {
    zoned(tz, ms).strftime("%H:%M").to_string()
}

/// `45 s`, `3 min`, `1 h 05 min`, `2 d 3 h`.
pub fn format_duration(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    let (d, h, m) = (s / 86_400, s / 3600 % 24, s / 60 % 60);
    match () {
        _ if d > 0 => format!("{d} d {h} h"),
        _ if h > 0 => format!("{h} h {m:02} min"),
        _ if m > 0 => format!("{m} min"),
        _ => format!("{s} s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    const HOUR: i64 = 3_600_000;

    fn ms(s: &str) -> i64 {
        s.parse::<Timestamp>().unwrap().as_millisecond()
    }

    #[test]
    fn days_follow_the_timezone_and_its_dst() {
        let paris = timezone("Europe/Paris");
        assert_eq!(
            day_start(&paris, date(2026, 10, 28)),
            ms("2026-10-27T23:00:00Z")
        );
        // The clocks go back on 2026-10-25: that day lasts 25 hours.
        let long_day =
            day_start(&paris, date(2026, 10, 26)) - day_start(&paris, date(2026, 10, 25));
        assert_eq!(long_day, 25 * HOUR);
        assert_eq!(
            day_of(&paris, ms("2026-10-27T23:30:00Z")),
            date(2026, 10, 28)
        );
        assert_eq!(
            day_of(&TimeZone::UTC, ms("2026-10-27T23:30:00Z")),
            date(2026, 10, 27)
        );
    }

    #[test]
    fn hours_start_on_the_local_hour() {
        let kolkata = timezone("Asia/Kolkata"); // UTC+05:30
        assert_eq!(
            hour_start(&kolkata, ms("2026-10-28T05:15:00Z")), // 10:45 local
            ms("2026-10-28T04:30:00Z")                        // 10:00 local
        );
    }

    #[test]
    fn day_keys_round_trip() {
        assert_eq!(day_key(date(2026, 3, 9)), 20260309);
        assert_eq!(day_from_key(20260309), Some(date(2026, 3, 9)));
        assert_eq!(day_from_key(20261332), None);
    }

    #[test]
    fn unknown_timezone_is_utc_and_formatting_is_local() {
        let tz = timezone("Mars/Olympus_Mons");
        assert_eq!(
            format_datetime(&tz, ms("2026-10-28T14:05:59Z")),
            "2026-10-28 14:05"
        );
        let paris = timezone("Europe/Paris");
        assert_eq!(
            format_datetime(&paris, ms("2026-10-28T14:05:00Z")),
            "2026-10-28 15:05"
        );
    }

    #[test]
    fn durations_keep_two_units_at_most() {
        assert_eq!(format_duration(45_000), "45 s");
        assert_eq!(format_duration(185_000), "3 min");
        assert_eq!(format_duration(3_900_000), "1 h 05 min");
        assert_eq!(
            format_duration(2 * 86_400_000 + 3 * 3_600_000 + 60_000),
            "2 d 3 h"
        );
    }
}
