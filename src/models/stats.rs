use crate::models::id_list;
use sqlx::{FromRow, SqlitePool};

/// Availability of one monitor over one time bucket, summed from its raw checks.
///
/// Each check records the time it covers (see `checks.covered_ms`) and whether that time was
/// downtime, so the downtime of an incident runs from its first failed check to the check that
/// resolves it, and paused periods count for nothing.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Bucket {
    pub monitor_id: i64,
    /// Position of the bucket in the window, from 0. Empty buckets are not returned.
    #[sqlx(rename = "idx")]
    pub index: i64,
    pub monitored_ms: i64,
    pub downtime_ms: i64,
    pub checks_total: i64,
    pub checks_failed: i64,
    pub latency_avg_ms: Option<i64>,
    /// Checks with a latency, i.e. that got a response.
    pub latency_count: i64,
}

impl Bucket {
    /// Buckets of `width` ms from `from` up to `to` (excluded), for these monitors or all of
    /// them, sorted by monitor then index.
    pub async fn query(
        pool: &SqlitePool,
        monitor_ids: Option<&[i64]>,
        from: i64,
        width: i64,
        to: i64,
    ) -> Result<Vec<Self>, sqlx::Error> {
        // Monitors drive the join so each one reads its checks as a range of the primary key.
        sqlx::query_as(
            "SELECT m.id AS monitor_id, (ch.checked_at - ?1) / ?2 AS idx,
                    SUM(ch.covered_ms) AS monitored_ms,
                    SUM(CASE WHEN ch.down THEN ch.covered_ms ELSE 0 END) AS downtime_ms,
                    COUNT(*) AS checks_total,
                    SUM(NOT ch.ok) AS checks_failed,
                    CAST(ROUND(AVG(ch.latency_ms)) AS INTEGER) AS latency_avg_ms,
                    COUNT(ch.latency_ms) AS latency_count
             FROM monitors m CROSS JOIN checks ch
                 ON ch.monitor_id = m.id AND ch.checked_at >= ?1 AND ch.checked_at < ?3
             WHERE ?4 IS NULL OR m.id IN (SELECT value FROM json_each(?4))
             GROUP BY m.id, idx ORDER BY m.id, idx",
        )
        .bind(from)
        .bind(width)
        .bind(to)
        .bind(monitor_ids.map(id_list))
        .fetch_all(pool)
        .await
    }

    /// `1 − downtime / monitored time`; `None` when nothing was monitored.
    pub fn uptime(&self) -> Option<f64> {
        uptime(self.monitored_ms, self.downtime_ms)
    }
}

pub fn uptime(monitored_ms: i64, downtime_ms: i64) -> Option<f64> {
    (monitored_ms > 0).then(|| 1.0 - downtime_ms as f64 / monitored_ms as f64)
}
