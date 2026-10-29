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
                    COALESCE((SELECT CASE WHEN ?2 - p.checked_at <= m.interval_s * 2000
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
