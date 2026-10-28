use crate::engine::verdict::CheckOutcome;
use sqlx::SqliteConnection;

/// Raw check results, short retention.
pub struct Check;

impl Check {
    pub async fn insert(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        outcome: &CheckOutcome,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO checks (monitor_id, checked_at, ok, latency_ms, status_code, error_kind, error)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
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
}
