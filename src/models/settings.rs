use sqlx::SqlitePool;

/// The single `settings` row, created by the migration.
pub struct Settings;

impl Settings {
    pub async fn block_private_targets(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
        let (block,): (bool,) =
            sqlx::query_as("SELECT block_private_targets FROM settings WHERE id = 1")
                .fetch_one(pool)
                .await?;
        Ok(block)
    }
}
