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
        let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM admin WHERE id = 1")
            .fetch_optional(pool)
            .await?;
        Ok(row.is_some())
    }

    /// First-launch setup: creates the admin and stores the instance timezone, atomically.
    /// Fails with a unique violation when the admin already exists.
    pub async fn setup(
        pool: &SqlitePool,
        username: &str,
        password_hash: &str,
        timezone: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO admin (id, username, password_hash, updated_at) VALUES (1, ?, ?, ?)",
        )
        .bind(username)
        .bind(password_hash)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE settings SET timezone = ? WHERE id = 1")
            .bind(timezone)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// Replaces `verified_hash` by `new_hash` and ends every session except `keep_token_hash`,
    /// atomically. `false`, and nothing changed, when the hash is no longer the one the caller
    /// verified or its session was ended meanwhile: a concurrent change won.
    pub async fn change_password(
        pool: &SqlitePool,
        verified_hash: &str,
        new_hash: &str,
        keep_token_hash: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE admin SET password_hash = ?, updated_at = ?
             WHERE id = 1 AND password_hash = ?
               AND EXISTS (SELECT 1 FROM sessions WHERE token_hash = ?)",
        )
        .bind(new_hash)
        .bind(now_ms())
        .bind(verified_hash)
        .bind(keep_token_hash)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Ok(false);
        }
        sqlx::query("DELETE FROM sessions WHERE token_hash <> ?")
            .bind(keep_token_hash)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
}
