use crate::engine::verdict::CheckOutcome;
use sqlx::SqliteConnection;

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
}
