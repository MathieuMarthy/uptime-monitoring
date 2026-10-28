use crate::auth::{SESSION_TTL_MS, random_token, sha256_hex};
use crate::time::now_ms;
use sqlx::SqlitePool;

/// A logged-in session, as seen by handlers. Only the SHA-256 of its token is stored.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub token_hash: String,
    pub csrf_token: String,
}

/// A session just created: the token goes into the cookie and is never stored.
pub struct NewSession {
    pub token: String,
    pub csrf_token: String,
}

impl Session {
    pub async fn create(pool: &SqlitePool) -> Result<NewSession, sqlx::Error> {
        let token = random_token();
        let csrf_token = random_token();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO sessions (token_hash, csrf_token, created_at, expires_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(sha256_hex(&token))
        .bind(&csrf_token)
        .bind(now)
        .bind(now + SESSION_TTL_MS)
        .execute(pool)
        .await?;
        Ok(NewSession { token, csrf_token })
    }

    /// The unexpired session for a cookie token.
    pub async fn find(pool: &SqlitePool, token: &str) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT token_hash, csrf_token FROM sessions WHERE token_hash = ? AND expires_at > ?",
        )
        .bind(sha256_hex(token))
        .bind(now_ms())
        .fetch_optional(pool)
        .await
    }

    pub async fn delete(&self, pool: &SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(&self.token_hash)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn purge_expired(pool: &SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
            .bind(now_ms())
            .execute(pool)
            .await?;
        Ok(())
    }
}
