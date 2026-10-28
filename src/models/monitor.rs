use crate::forms::monitor::MonitorInput;
use crate::models::MonitorState;
use crate::time::now_ms;
use sqlx::{FromRow, SqlitePool};

const COLUMNS: &str = "id, name, kind, url, method, headers, body, keyword, accepted_statuses, \
    follow_redirects, ignore_tls_errors, interval_s, retry_interval_s, timeout_s, failure_threshold, tls_expiry_warn_days, paused, \
    created_at, updated_at";

/// A monitor's configuration, written only by the admin.
#[derive(Debug, Clone, FromRow)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub method: String,
    pub headers: Option<String>,
    pub body: Option<String>,
    pub keyword: Option<String>,
    pub accepted_statuses: String,
    pub follow_redirects: bool,
    pub ignore_tls_errors: bool,
    pub interval_s: i64,
    pub retry_interval_s: i64,
    pub timeout_s: i64,
    pub failure_threshold: i64,
    pub tls_expiry_warn_days: Option<i64>,
    pub paused: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Monitor {
    pub async fn find_all(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM monitors ORDER BY name COLLATE NOCASE"
        )))
        .fetch_all(pool)
        .await
    }

    pub async fn find(pool: &SqlitePool, id: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM monitors WHERE id = ?"
        )))
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    /// Inserts the monitor and its initial `pending` state in one transaction.
    pub async fn create(pool: &SqlitePool, input: &MonitorInput) -> Result<i64, sqlx::Error> {
        let now = now_ms();
        let mut tx = pool.begin().await?;

        let id = sqlx::query(
            r#"
            INSERT INTO monitors (
                name, url, method, headers, body, keyword, accepted_statuses, follow_redirects,
                ignore_tls_errors, interval_s, retry_interval_s, timeout_s, failure_threshold,
                tls_expiry_warn_days, paused, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&input.name)
        .bind(&input.url)
        .bind(&input.method)
        .bind(&input.headers)
        .bind(&input.body)
        .bind(&input.keyword)
        .bind(&input.accepted_statuses)
        .bind(input.follow_redirects)
        .bind(input.ignore_tls_errors)
        .bind(input.interval_s)
        .bind(input.retry_interval_s)
        .bind(input.timeout_s)
        .bind(input.failure_threshold)
        .bind(input.tls_expiry_warn_days)
        .bind(input.paused)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();

        sqlx::query("INSERT INTO monitor_state (monitor_id) VALUES (?)")
            .bind(id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(id)
    }

    /// Returns `false` when no monitor has this id.
    pub async fn update(
        pool: &SqlitePool,
        id: i64,
        input: &MonitorInput,
    ) -> Result<bool, sqlx::Error> {
        let now = now_ms();
        let mut tx = pool.begin().await?;
        let result = sqlx::query(
            r#"
            UPDATE monitors
            SET name = ?, url = ?, method = ?, headers = ?, body = ?, keyword = ?,
                accepted_statuses = ?, follow_redirects = ?, ignore_tls_errors = ?, interval_s = ?,
                retry_interval_s = ?, timeout_s = ?, failure_threshold = ?, tls_expiry_warn_days = ?, paused = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&input.name)
        .bind(&input.url)
        .bind(&input.method)
        .bind(&input.headers)
        .bind(&input.body)
        .bind(&input.keyword)
        .bind(&input.accepted_statuses)
        .bind(input.follow_redirects)
        .bind(input.ignore_tls_errors)
        .bind(input.interval_s)
        .bind(input.retry_interval_s)
        .bind(input.timeout_s)
        .bind(input.failure_threshold)
        .bind(input.tls_expiry_warn_days)
        .bind(input.paused)
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }

        if input.paused {
            MonitorState::reset_for_pause(&mut tx, id, now).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Returns `false` when no monitor has this id.
    pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM monitors WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Returns `false` when no monitor has this id. Pausing resolves the open incident and resets
    /// the state to `pending`.
    pub async fn toggle_pause(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
        let now = now_ms();
        let mut tx = pool.begin().await?;
        let paused: Option<(bool,)> = sqlx::query_as(
            "UPDATE monitors SET paused = 1 - paused, updated_at = ? WHERE id = ? RETURNING paused",
        )
        .bind(now)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((paused,)) = paused else {
            return Ok(false);
        };

        if paused {
            MonitorState::reset_for_pause(&mut tx, id, now).await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}
