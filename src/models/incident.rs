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
        sqlx::query_as(
            "SELECT id, monitor_id, started_at, resolved_at, error_kind, cause FROM incidents
             WHERE monitor_id = ? ORDER BY started_at DESC",
        )
        .bind(monitor_id)
        .fetch_all(pool)
        .await
    }

    /// Opens an incident starting at the failed check recorded at `since`, with its error.
    pub async fn open(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        since: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO incidents (monitor_id, started_at, error_kind, cause)
             SELECT monitor_id, checked_at, error_kind, error FROM checks
             WHERE monitor_id = ? AND checked_at = ?",
        )
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
}
