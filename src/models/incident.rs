use chrono::DateTime;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Incident {
    pub id: String,
    pub service_id: String,
    pub started_at: i64,
    pub resolved_at: Option<i64>,
    pub cause: Option<String>,
    pub error_type: String,
}

impl Incident {
    pub async fn find_recent_by_service(
        pool: &SqlitePool,
        service_id: &str,
        limit: i64,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT id, service_id, started_at, resolved_at, cause, error_type
            FROM incidents
            WHERE service_id = ?
            ORDER BY started_at DESC
            LIMIT ?
            "#,
        )
        .bind(service_id)
        .bind(limit)
        .fetch_all(pool)
        .await
    }

    pub fn started_at_formatted(&self) -> String {
        match DateTime::from_timestamp(self.started_at, 0) {
            Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
            None => format!("{}", self.started_at),
        }
    }

    pub fn started_at_relative(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let diff = (now - self.started_at).max(0);

        if diff < 60 {
            "Just now".to_string()
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else if diff < 86400 {
            format!("{}h ago", diff / 3600)
        } else {
            format!("{}d ago", diff / 86400)
        }
    }

    pub fn duration_formatted(&self) -> String {
        let end_time = match self.resolved_at {
            Some(resolved) => resolved,
            None => return "Ongoing".to_string(),
        };

        let duration_secs = (end_time - self.started_at).max(0);
        if duration_secs < 60 {
            format!("{}s", duration_secs)
        } else if duration_secs < 3600 {
            format!("{}m", duration_secs / 60)
        } else {
            let hours = duration_secs / 3600;
            let mins = (duration_secs % 3600) / 60;
            format!("{}h {}m", hours, mins)
        }
    }
}
