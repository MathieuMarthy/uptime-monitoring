use sqlx::{FromRow, SqlitePool};

const COLUMNS: &str = "monitor_id, status, consecutive_failures, last_check_at, last_latency_ms, \
    last_error, tls_expires_at, tls_warned_for";

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum MonitorStatus {
    Pending,
    Up,
    Down,
}

/// A monitor's runtime state, written only by the check engine.
#[derive(Debug, Clone, FromRow)]
pub struct MonitorState {
    pub monitor_id: i64,
    pub status: MonitorStatus,
    pub consecutive_failures: i64,
    pub last_check_at: Option<i64>,
    pub last_latency_ms: Option<i64>,
    pub last_error: Option<String>,
    pub tls_expires_at: Option<i64>,
    pub tls_warned_for: Option<i64>,
}

impl MonitorState {
    pub async fn find_all(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM monitor_state"
        )))
        .fetch_all(pool)
        .await
    }

    pub async fn find(pool: &SqlitePool, monitor_id: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM monitor_state WHERE monitor_id = ?"
        )))
        .bind(monitor_id)
        .fetch_optional(pool)
        .await
    }
}
