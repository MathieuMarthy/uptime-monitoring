use crate::models::{Bucket, id_list};
use sqlx::{FromRow, SqlitePool};

/// One monitor's availability over one local day, rolled up from its raw checks.
#[derive(Debug, Clone, FromRow)]
pub struct DailyStat {
    pub monitor_id: i64,
    /// `YYYYMMDD`, see `time::day_key`.
    pub day: i64,
    pub monitored_ms: i64,
    pub downtime_ms: i64,
    pub checks_total: i64,
    pub checks_failed: i64,
    pub latency_avg_ms: Option<i64>,
    pub latency_p95_ms: Option<i64>,
}

impl DailyStat {
    /// The last day rolled up, for any monitor.
    pub async fn last_day(pool: &SqlitePool) -> Result<Option<i64>, sqlx::Error> {
        let (day,) = sqlx::query_as("SELECT MAX(day) FROM daily_stats")
            .fetch_one(pool)
            .await?;
        Ok(day)
    }

    /// The days of these monitors from `first_day` on, by monitor then oldest first.
    pub async fn for_monitors(
        pool: &SqlitePool,
        monitor_ids: &[i64],
        first_day: i64,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT monitor_id, day, monitored_ms, downtime_ms, checks_total, checks_failed,
                    latency_avg_ms, latency_p95_ms
             FROM daily_stats
             WHERE monitor_id IN (SELECT value FROM json_each(?)) AND day >= ?
             ORDER BY monitor_id, day",
        )
        .bind(id_list(monitor_ids))
        .bind(first_day)
        .fetch_all(pool)
        .await
    }

    /// Rolls up the checks of `[from, to)` as `day`, for every monitor that has some. A day
    /// already rolled up is left as it is.
    pub async fn roll_up(
        pool: &SqlitePool,
        day: i64,
        from: i64,
        to: i64,
    ) -> Result<(), sqlx::Error> {
        let mut rows = Vec::new();
        for b in Bucket::query(pool, None, from, to - from, to).await? {
            let p95 = p95(pool, b.monitor_id, from, to, b.latency_count).await?;
            rows.push((b, p95));
        }
        let mut tx = pool.begin().await?;
        for (b, p95) in rows {
            sqlx::query(
                "INSERT INTO daily_stats (monitor_id, day, monitored_ms, downtime_ms, checks_total,
                                          checks_failed, latency_avg_ms, latency_p95_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT DO NOTHING",
            )
            .bind(b.monitor_id)
            .bind(day)
            .bind(b.monitored_ms)
            .bind(b.downtime_ms)
            .bind(b.checks_total)
            .bind(b.checks_failed)
            .bind(b.latency_avg_ms)
            .bind(p95)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    pub async fn purge_before(pool: &SqlitePool, day: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM daily_stats WHERE day < ?")
            .bind(day)
            .execute(pool)
            .await?;
        Ok(())
    }
}

/// The nearest-rank 95th percentile of the `count` latencies of a monitor over `[from, to)`.
async fn p95(
    pool: &SqlitePool,
    monitor_id: i64,
    from: i64,
    to: i64,
    count: i64,
) -> Result<Option<i64>, sqlx::Error> {
    if count == 0 {
        return Ok(None);
    }
    let rank = (count * 95 + 99) / 100;
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT latency_ms FROM checks
         WHERE monitor_id = ? AND checked_at >= ? AND checked_at < ? AND latency_ms IS NOT NULL
         ORDER BY latency_ms LIMIT 1 OFFSET ?",
    )
    .bind(monitor_id)
    .bind(from)
    .bind(to)
    .bind(rank - 1)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(latency,)| latency))
}
