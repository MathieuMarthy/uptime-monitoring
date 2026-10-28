use crate::time::now_ms;
use sqlx::SqlitePool;

/// The single administrator account (`id = 1`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Admin {
    pub username: String,
    pub password_hash: String,
}

impl Admin {
    pub async fn find(pool: &SqlitePool) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT username, password_hash FROM admin WHERE id = 1")
            .fetch_optional(pool)
            .await
    }

    pub async fn exists(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
        Ok(Self::find(pool).await?.is_some())
    }

    /// Fails with a unique violation when the admin already exists.
    pub async fn create(
        pool: &SqlitePool,
        username: &str,
        password_hash: &str,
    ) -> Result<(), sqlx::Error> {
        Self::insert(pool, username, password_hash).await
    }

    /// First-launch setup: creates the admin and stores the instance timezone, atomically.
    pub async fn setup(
        pool: &SqlitePool,
        username: &str,
        password_hash: &str,
        timezone: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = pool.begin().await?;
        Self::insert(&mut *tx, username, password_hash).await?;
        sqlx::query("UPDATE settings SET timezone = ? WHERE id = 1")
            .bind(timezone)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    async fn insert(
        executor: impl sqlx::SqliteExecutor<'_>,
        username: &str,
        password_hash: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO admin (id, username, password_hash, updated_at) VALUES (1, ?, ?, ?)",
        )
        .bind(username)
        .bind(password_hash)
        .bind(now_ms())
        .execute(executor)
        .await?;
        Ok(())
    }
}
