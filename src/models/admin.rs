use crate::time::now_ms;
use sqlx::SqlitePool;

/// The single administrator account (`id = 1`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Admin {
    pub username: String,
    pub password_hash: String,
    /// Base32 TOTP secret; `None` = two-factor authentication off.
    pub totp_secret: Option<String>,
    pub totp_last_step: Option<i64>,
}

impl Admin {
    pub async fn find(pool: &SqlitePool) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT username, password_hash, totp_secret, totp_last_step FROM admin WHERE id = 1",
        )
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

    /// Turns two-factor authentication on with `secret`, whose code of `step` the admin just
    /// typed, and ends every session except `keep_token_hash`, atomically. `false`, and nothing
    /// changed, when that session was ended meanwhile.
    pub async fn enable_totp(
        pool: &SqlitePool,
        secret: &str,
        step: i64,
        keep_token_hash: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE admin SET totp_secret = ?, totp_last_step = ?, updated_at = ?
             WHERE id = 1 AND EXISTS (SELECT 1 FROM sessions WHERE token_hash = ?)",
        )
        .bind(secret)
        .bind(step)
        .bind(now_ms())
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

    pub async fn disable_totp(pool: &SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE admin SET totp_secret = NULL, totp_last_step = NULL, updated_at = ? WHERE id = 1",
        )
        .bind(now_ms())
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Records `step` as used for `secret`. `false` when that step or a later one was already
    /// used, or the secret changed: a concurrent login took the code first.
    pub async fn use_totp_step(
        pool: &SqlitePool,
        secret: &str,
        step: i64,
    ) -> Result<bool, sqlx::Error> {
        let updated = sqlx::query(
            "UPDATE admin SET totp_last_step = ?
             WHERE id = 1 AND totp_secret = ? AND (totp_last_step IS NULL OR totp_last_step < ?)",
        )
        .bind(step)
        .bind(secret)
        .bind(step)
        .execute(pool)
        .await?
        .rows_affected();
        Ok(updated == 1)
    }
}
