use crate::engine::verdict::CheckOutcome;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

/// A raw check result, as listed on a monitor's page.
#[derive(Debug, Clone, FromRow)]
pub struct RecentCheck {
    pub checked_at: i64,
    pub ok: bool,
    pub latency_ms: Option<i64>,
    pub status_code: Option<i64>,
    pub error: Option<String>,
}

/// A failed check, with the number of failures matched by the query that returned it.
#[derive(Debug, Clone, FromRow)]
pub struct FailedCheck {
    pub monitor_id: i64,
    pub monitor_name: String,
    pub checked_at: i64,
    pub latency_ms: Option<i64>,
    pub status_code: Option<i64>,
    pub error_kind: Option<String>,
    pub error: Option<String>,
    pub total: i64,
}

/// Raw check results, short retention. Each check also records the time it covers, for
/// uptime: see `Bucket`.
pub struct Check;

impl Check {
    pub async fn insert(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        outcome: &CheckOutcome,
    ) -> Result<(), sqlx::Error> {
        // The previous check is the last one recorded; an incident still open covers it.
        sqlx::query(
            "INSERT INTO checks
                 (monitor_id, checked_at, ok, latency_ms, status_code, error_kind, error, covered_ms,
                  down)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    COALESCE((SELECT CASE WHEN ?2 - p.checked_at <= (m.interval_s + m.grace_s) * 2000
                                          THEN ?2 - p.checked_at ELSE 0 END
                              FROM checks p JOIN monitors m ON m.id = p.monitor_id
                              WHERE p.monitor_id = ?1 AND p.checked_at < ?2
                              ORDER BY p.checked_at DESC LIMIT 1), 0),
                    EXISTS (SELECT 1 FROM incidents WHERE monitor_id = ?1 AND resolved_at IS NULL)",
        )
        .bind(monitor_id)
        .bind(outcome.checked_at)
        .bind(outcome.ok())
        .bind(outcome.latency_ms)
        .bind(outcome.status_code)
        .bind(outcome.failure.as_ref().map(|f| f.kind.as_str()))
        .bind(outcome.failure.as_ref().map(|f| f.message.as_str()))
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The `limit` most recent checks of a monitor, first.
    pub async fn recent(
        pool: &SqlitePool,
        monitor_id: i64,
        limit: i64,
    ) -> Result<Vec<RecentCheck>, sqlx::Error> {
        sqlx::query_as(
            "SELECT checked_at, ok, latency_ms, status_code, error FROM checks
             WHERE monitor_id = ? ORDER BY checked_at DESC LIMIT ?",
        )
        .bind(monitor_id)
        .bind(limit)
        .fetch_all(pool)
        .await
    }

    /// The `limit` latest failures of a monitor between `from` and `to` (both included), first;
    /// each row also gives how many failures there are in that window.
    pub async fn failures_between(
        pool: &SqlitePool,
        monitor_id: i64,
        from: i64,
        to: i64,
        limit: i64,
    ) -> Result<Vec<FailedCheck>, sqlx::Error> {
        sqlx::query_as(
            "SELECT c.monitor_id, m.name AS monitor_name, c.checked_at, c.latency_ms, c.status_code,
                    c.error_kind, c.error, COUNT(*) OVER () AS total
             FROM checks c JOIN monitors m ON m.id = c.monitor_id
             WHERE c.monitor_id = ? AND c.checked_at BETWEEN ? AND ? AND c.ok = 0
             ORDER BY c.checked_at DESC LIMIT ?",
        )
        .bind(monitor_id)
        .bind(from)
        .bind(to)
        .bind(limit)
        .fetch_all(pool)
        .await
    }

    /// Up to `limit` failures from `from` to `before` that did not count as downtime: a retry
    /// that passed, or a series still below its threshold. Latest first, for one monitor or all of
    /// them; those at `before` itself are kept when their monitor id is below `tie_id`.
    pub async fn isolated_failures(
        pool: &SqlitePool,
        monitor_id: Option<i64>,
        from: i64,
        before: i64,
        tie_id: i64,
        limit: i64,
    ) -> Result<Vec<FailedCheck>, sqlx::Error> {
        // The first failure of an incident is not marked as downtime: the incident stands for it.
        sqlx::query_as(
            "SELECT c.monitor_id, m.name AS monitor_name, c.checked_at, c.latency_ms, c.status_code,
                    c.error_kind, c.error, 1 AS total
             FROM checks c JOIN monitors m ON m.id = c.monitor_id
             WHERE c.ok = 0 AND c.down = 0 AND c.checked_at >= ?1
               AND (c.checked_at < ?2 OR (c.checked_at = ?2 AND c.monitor_id < ?5))
               AND (?3 IS NULL OR c.monitor_id = ?3)
               AND NOT EXISTS (SELECT 1 FROM incidents i
                               WHERE i.monitor_id = c.monitor_id AND i.started_at = c.checked_at)
             ORDER BY c.checked_at DESC, c.monitor_id DESC LIMIT ?4",
        )
        .bind(from)
        .bind(before)
        .bind(monitor_id)
        .bind(limit)
        .bind(tie_id)
        .fetch_all(pool)
        .await
    }

    /// The time of the oldest check kept, across monitors.
    pub async fn oldest(pool: &SqlitePool) -> Result<Option<i64>, sqlx::Error> {
        let (oldest,) = sqlx::query_as(
            "SELECT MIN((SELECT MIN(checked_at) FROM checks WHERE monitor_id = m.id))
             FROM monitors m",
        )
        .fetch_one(pool)
        .await?;
        Ok(oldest)
    }

    pub async fn purge_before(pool: &SqlitePool, cutoff: i64) -> Result<(), sqlx::Error> {
        // Through the monitors, so each one deletes a range of the primary key.
        sqlx::query(
            "DELETE FROM checks WHERE monitor_id IN (SELECT id FROM monitors) AND checked_at < ?",
        )
        .bind(cutoff)
        .execute(pool)
        .await?;
        Ok(())
    }
}
