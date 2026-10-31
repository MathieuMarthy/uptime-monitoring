use sqlx::{FromRow, SqliteConnection, SqlitePool};

/// A period when a monitor was degraded: up, but slower than its latency threshold.
#[derive(Debug, Clone, FromRow)]
pub struct Slowdown {
    pub id: i64,
    pub monitor_id: i64,
    /// The first slow check of the series.
    pub started_at: i64,
    /// `None` while it lasts.
    pub resolved_at: Option<i64>,
    /// The threshold in force when it started.
    pub threshold_ms: i64,
    /// Its start was alerted, so its end is too.
    pub alerted: bool,
}

/// A slowdown with its monitor's name, for the Activity page.
#[derive(Debug, Clone, FromRow)]
pub struct NamedSlowdown {
    #[sqlx(flatten)]
    pub slowdown: Slowdown,
    pub monitor_name: String,
    /// When the monitor's next slowdown started: alerts from then on are that one's.
    pub next_started_at: Option<i64>,
}

impl Slowdown {
    /// Up to `limit` slowdowns started from `from` to `before`, latest first; those started at
    /// `before` itself are kept when their id is below `tie_id`.
    pub async fn started_between(
        pool: &SqlitePool,
        monitor_id: Option<i64>,
        from: i64,
        before: i64,
        tie_id: i64,
        limit: i64,
    ) -> Result<Vec<NamedSlowdown>, sqlx::Error> {
        sqlx::query_as(
            "SELECT s.id, s.monitor_id, s.started_at, s.resolved_at, s.threshold_ms, s.alerted,
                    m.name AS monitor_name,
                    (SELECT MIN(n.started_at) FROM slowdowns n
                     WHERE n.monitor_id = s.monitor_id AND n.started_at > s.started_at)
                        AS next_started_at
             FROM slowdowns s JOIN monitors m ON m.id = s.monitor_id
             WHERE s.started_at >= ?1 AND (s.started_at < ?2 OR (s.started_at = ?2 AND s.id < ?5))
               AND (?3 IS NULL OR s.monitor_id = ?3)
             ORDER BY s.started_at DESC, s.id DESC LIMIT ?4",
        )
        .bind(from)
        .bind(before)
        .bind(monitor_id)
        .bind(limit)
        .bind(tie_id)
        .fetch_all(pool)
        .await
    }

    pub async fn open(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        since: i64,
        threshold_ms: i64,
        alerted: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO slowdowns (monitor_id, started_at, threshold_ms, alerted)
             VALUES (?, ?, ?, ?)",
        )
        .bind(monitor_id)
        .bind(since)
        .bind(threshold_ms)
        .bind(alerted)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Ends the open slowdown, if any; returns whether its start was alerted.
    pub async fn resolve(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        at: i64,
    ) -> Result<bool, sqlx::Error> {
        let alerted: Option<bool> = sqlx::query_scalar(
            "UPDATE slowdowns SET resolved_at = ? WHERE monitor_id = ? AND resolved_at IS NULL
             RETURNING alerted",
        )
        .bind(at)
        .bind(monitor_id)
        .fetch_optional(conn)
        .await?;
        Ok(alerted.unwrap_or(false))
    }

    /// Deletes the slowdowns resolved before `cutoff`; an open one stays, however old.
    pub async fn purge_resolved_before(pool: &SqlitePool, cutoff: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM slowdowns WHERE resolved_at < ?")
            .bind(cutoff)
            .execute(pool)
            .await?;
        Ok(())
    }
}
