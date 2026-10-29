use sqlx::{FromRow, SqlitePool};

/// One delivery: the final result after retries.
pub struct Delivery<'a> {
    pub channel_id: i64,
    pub monitor_id: Option<i64>,
    pub event: &'static str,
    pub sent_at: i64,
    pub error: Option<&'a str>,
}

/// A row of the delivery log, with the names of its channel and monitor (`None` once deleted).
#[derive(Debug, Clone, FromRow)]
pub struct LogEntry {
    pub sent_at: i64,
    pub channel_name: Option<String>,
    pub monitor_name: Option<String>,
    pub event: String,
    pub ok: bool,
    pub error: Option<String>,
}

pub struct NotificationLog;

impl NotificationLog {
    pub async fn insert(pool: &SqlitePool, delivery: &Delivery<'_>) -> Result<(), sqlx::Error> {
        sqlx::query(
            // The channel or monitor may have been deleted during the retries.
            "INSERT INTO notification_log (channel_id, monitor_id, event, sent_at, ok, error)
             VALUES ((SELECT id FROM notification_channels WHERE id = ?),
                     (SELECT id FROM monitors WHERE id = ?), ?, ?, ?, ?)",
        )
        .bind(delivery.channel_id)
        .bind(delivery.monitor_id)
        .bind(delivery.event)
        .bind(delivery.sent_at)
        .bind(delivery.error.is_none())
        .bind(delivery.error)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// The `limit` most recent deliveries, first.
    pub async fn recent(pool: &SqlitePool, limit: i64) -> Result<Vec<LogEntry>, sqlx::Error> {
        sqlx::query_as(
            "SELECT l.sent_at, c.name AS channel_name, m.name AS monitor_name, l.event, l.ok, l.error
             FROM notification_log l
             LEFT JOIN notification_channels c ON c.id = l.channel_id
             LEFT JOIN monitors m ON m.id = l.monitor_id
             ORDER BY l.id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await
    }
}
