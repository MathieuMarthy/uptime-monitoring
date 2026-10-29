use crate::engine::verdict::Failure;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

/// An outage period: from the first failed check of the series to the first successful check.
#[derive(Debug, Clone, FromRow)]
pub struct Incident {
    pub id: i64,
    pub monitor_id: i64,
    pub started_at: i64,
    pub resolved_at: Option<i64>,
    pub error_kind: String,
    pub cause: Option<String>,
}

impl Incident {
    /// Most recent first.
    pub async fn for_monitor(pool: &SqlitePool, monitor_id: i64) -> Result<Vec<Self>, sqlx::Error> {
        Self::recent(pool, monitor_id, -1).await
    }

    /// The `limit` most recent, first; a negative limit returns them all.
    pub async fn recent(
        pool: &SqlitePool,
        monitor_id: i64,
        limit: i64,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, monitor_id, started_at, resolved_at, error_kind, cause FROM incidents
             WHERE monitor_id = ? ORDER BY started_at DESC LIMIT ?",
        )
        .bind(monitor_id)
        .bind(limit)
        .fetch_all(pool)
        .await
    }

    /// Opens an incident starting at the failed check recorded at `since`, with its error, and
    /// marks the checks after it as downtime. If that check was already purged, `latest` (the
    /// failure that reached the threshold) gives the error instead.
    pub async fn open(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        since: i64,
        latest: &Failure,
    ) -> Result<(), sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO incidents (monitor_id, started_at, error_kind, cause)
             SELECT monitor_id, checked_at, error_kind, error FROM checks
             WHERE monitor_id = ? AND checked_at = ?",
        )
        .bind(monitor_id)
        .bind(since)
        .execute(&mut *conn)
        .await?;
        if inserted.rows_affected() == 0 {
            sqlx::query(
                "INSERT INTO incidents (monitor_id, started_at, error_kind, cause) VALUES (?, ?, ?, ?)",
            )
            .bind(monitor_id)
            .bind(since)
            .bind(latest.kind.as_str())
            .bind(&latest.message)
            .execute(&mut *conn)
            .await?;
        }
        // The failures before the threshold was reached were downtime too.
        sqlx::query("UPDATE checks SET down = 1 WHERE monitor_id = ? AND checked_at > ?")
            .bind(monitor_id)
            .bind(since)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Resolves the open incident, if any.
    pub async fn resolve(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        at: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE incidents SET resolved_at = ? WHERE monitor_id = ? AND resolved_at IS NULL",
        )
        .bind(at)
        .bind(monitor_id)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Deletes the incidents resolved before `cutoff`; an open incident stays, however old.
    pub async fn purge_resolved_before(pool: &SqlitePool, cutoff: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM incidents WHERE resolved_at < ?")
            .bind(cutoff)
            .execute(pool)
            .await?;
        Ok(())
    }
}
