use crate::i18n::Lang;
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
    /// Code of the interface and notification language.
    pub language: String,
}

impl Settings {
    pub async fn load(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        sqlx::query_as(
            "SELECT timezone, checks_retention_days, history_retention_days, block_private_targets,
                    language
             FROM settings WHERE id = 1",
        )
        .fetch_one(pool)
        .await
    }

    pub async fn update_general(
        pool: &SqlitePool,
        timezone: &str,
        language: Lang,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE settings SET timezone = ?, language = ? WHERE id = 1")
            .bind(timezone)
            .bind(language.code())
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn update_retention(
        pool: &SqlitePool,
        checks_retention_days: i64,
        history_retention_days: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE settings SET checks_retention_days = ?, history_retention_days = ? WHERE id = 1",
        )
        .bind(checks_retention_days)
        .bind(history_retention_days)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn update_blocking(
        pool: &SqlitePool,
        block_private_targets: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE settings SET block_private_targets = ? WHERE id = 1")
            .bind(block_private_targets)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// The language; English if the stored code is unknown.
    pub fn lang(&self) -> Lang {
        Lang::from_code(&self.language).unwrap_or_default()
    }

    pub fn tz(&self) -> TimeZone {
        time::timezone(&self.timezone)
    }
}

/// What the storage estimate of the settings page is based on.
#[derive(Debug, Clone, Copy, PartialEq, FromRow)]
pub struct StorageUse {
    /// Checks a day of all monitors that are not paused (failure retries left out).
    pub checks_per_day: f64,
    /// Monitors that are not paused: one daily stat each per day.
    pub monitors: i64,
    /// Current size of the database file, free pages included.
    pub database_bytes: i64,
}

impl StorageUse {
    pub async fn load(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        sqlx::query_as(
            "SELECT COALESCE(SUM(86400.0 / interval_s), 0.0) AS checks_per_day,
                    COUNT(*) AS monitors,
                    (SELECT page_count * page_size FROM pragma_page_count, pragma_page_size)
                        AS database_bytes
             FROM monitors WHERE NOT paused",
        )
        .fetch_one(pool)
        .await
    }
}
