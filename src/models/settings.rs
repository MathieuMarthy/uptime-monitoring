use crate::time;
use jiff::tz::TimeZone;
use sqlx::{FromRow, SqlitePool};

/// The single `settings` row, created by the migration.
#[derive(Debug, Clone, FromRow)]
pub struct Settings {
    pub timezone: String,
    pub checks_retention_days: i64,
    /// `None`: incidents and daily stats are kept forever.
    pub history_retention_days: Option<i64>,
    pub block_private_targets: bool,
}

impl Settings {
    pub async fn load(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        sqlx::query_as(
            "SELECT timezone, checks_retention_days, history_retention_days, block_private_targets
             FROM settings WHERE id = 1",
        )
        .fetch_one(pool)
        .await
    }

    pub fn tz(&self) -> TimeZone {
        time::timezone(&self.timezone)
    }
}
