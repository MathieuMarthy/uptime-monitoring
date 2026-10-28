//! Hourly upkeep: rolls completed days up into `daily_stats`, then purges what outlived its
//! retention.

use crate::models::{Check, DailyStat, Incident, Session, Settings};
use crate::time::{self, day_from_key, day_key, day_of, day_start, now_ms};
use jiff::tz::TimeZone;
use sqlx::SqlitePool;
use std::time::Duration;

const PERIOD: Duration = Duration::from_secs(3600);
const DAY_MS: i64 = 24 * 3600 * 1000;
/// A day is rolled up this long after it ends, once the checks running at midnight are recorded.
const ROLLUP_DELAY_MS: i64 = 5 * 60 * 1000;

/// Runs the upkeep now, then every hour.
pub fn start(pool: SqlitePool) {
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(PERIOD);
        loop {
            ticks.tick().await;
            if let Err(e) = run(&pool, now_ms()).await {
                tracing::error!("maintenance failed: {e}");
            }
        }
    });
}

pub async fn run(pool: &SqlitePool, now: i64) -> Result<(), sqlx::Error> {
    let settings = Settings::load(pool).await?;
    let tz = settings.tz();
    roll_up(pool, &tz, now).await?;
    purge(pool, &settings, &tz, now).await
}

/// Rolls up every completed day since the last one rolled up, from the oldest check kept.
async fn roll_up(pool: &SqlitePool, tz: &TimeZone, now: i64) -> Result<(), sqlx::Error> {
    let Some(oldest) = Check::oldest(pool).await? else {
        return Ok(());
    };
    let Ok(last) = day_of(tz, now - ROLLUP_DELAY_MS).yesterday() else {
        return Ok(());
    };
    let mut day = day_of(tz, oldest);
    if let Some(done) = DailyStat::last_day(pool).await?.and_then(day_from_key)
        && let Ok(next) = done.tomorrow()
    {
        day = day.max(next);
    }
    while day <= last {
        let Ok(next) = day.tomorrow() else { break };
        DailyStat::roll_up(pool, day_key(day), day_start(tz, day), day_start(tz, next)).await?;
        day = next;
    }
    Ok(())
}

async fn purge(
    pool: &SqlitePool,
    settings: &Settings,
    tz: &TimeZone,
    now: i64,
) -> Result<(), sqlx::Error> {
    let checks_cutoff = now - settings.checks_retention_days * DAY_MS;
    Check::purge_before(pool, checks_cutoff).await?;
    // No model owns the notification log yet (alerts come later); it shares the checks' retention.
    sqlx::query("DELETE FROM notification_log WHERE sent_at < ?")
        .bind(checks_cutoff)
        .execute(pool)
        .await?;

    if let Some(days) = settings.history_retention_days {
        let cutoff = now - days * DAY_MS;
        Incident::purge_resolved_before(pool, cutoff).await?;
        DailyStat::purge_before(pool, time::day_key(day_of(tz, cutoff))).await?;
    }
    Session::purge_expired(pool, now).await
}
