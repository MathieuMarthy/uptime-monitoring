use crate::auth::random_token;
use crate::forms::monitor::{MonitorInput, PUSH};
use crate::models::{Channel, MonitorState, MonitorStatus};
use crate::time::now_ms;
use sqlx::{FromRow, SqlitePool};
use unicode_normalization::UnicodeNormalization;

const COLUMNS: &str = "id, name, group_name, kind, url, method, headers, body, keyword, accepted_statuses, \
    follow_redirects, ignore_tls_errors, interval_s, retry_interval_s, timeout_s, failure_threshold, tls_expiry_warn_days, paused, \
    created_at, updated_at, push_token, grace_s";

/// A monitor's configuration, written only by the admin.
#[derive(Debug, Clone, FromRow)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub group_name: Option<String>,
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
    /// Push monitors only: the secret part of their `/push/{token}` URL.
    pub push_token: Option<String>,
    /// Push monitors only: how late a ping may be; 0 for HTTP.
    pub grace_s: i64,
}

/// What the dashboard shows of a monitor: no request body or headers.
#[derive(Debug, Clone, FromRow)]
pub struct MonitorSummary {
    pub id: i64,
    pub name: String,
    pub group_name: Option<String>,
    pub url: String,
    pub paused: bool,
    pub status: MonitorStatus,
    pub last_latency_ms: Option<i64>,
}

impl Monitor {
    /// A monitor that is not stored, for a one-off check of a form's values. Both structs are
    /// spelled out whole: a field added to either one fails to compile here until it is mapped.
    pub fn unsaved(input: &MonitorInput) -> Self {
        let MonitorInput {
            name,
            group_name,
            kind,
            url,
            method,
            headers,
            body,
            keyword,
            accepted_statuses,
            follow_redirects,
            ignore_tls_errors,
            interval_s,
            retry_interval_s,
            timeout_s,
            failure_threshold,
            tls_expiry_warn_days,
            grace_s,
            paused,
            channel_ids: _,
        } = input.clone();
        Self {
            id: 0,
            name,
            group_name,
            kind,
            url,
            method,
            headers,
            body,
            keyword,
            accepted_statuses,
            follow_redirects,
            ignore_tls_errors,
            interval_s,
            retry_interval_s,
            timeout_s,
            failure_threshold,
            tls_expiry_warn_days,
            paused,
            created_at: 0,
            updated_at: 0,
            push_token: None,
            grace_s,
        }
    }

    /// The monitored job pings the instance instead of being checked; `url` is empty.
    pub fn is_push(&self) -> bool {
        self.kind == PUSH
    }

    /// The push monitor behind a ping URL, and whether it is paused; `None` for an unknown
    /// token.
    pub async fn find_push(
        pool: &SqlitePool,
        token: &str,
    ) -> Result<Option<(i64, bool)>, sqlx::Error> {
        sqlx::query_as("SELECT id, paused FROM monitors WHERE push_token = ?")
            .bind(token)
            .fetch_optional(pool)
            .await
    }

    /// Gives a push monitor a new token, which voids the old URL. Returns `false` when no push
    /// monitor has this id.
    pub async fn renew_push_token(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE monitors SET push_token = ?, updated_at = ? WHERE id = ? AND kind = 'push'",
        )
        .bind(random_token())
        .bind(now_ms())
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
    /// Every monitor with its state, sorted by name.
    pub async fn summaries(pool: &SqlitePool) -> Result<Vec<MonitorSummary>, sqlx::Error> {
        sqlx::query_as(
            "SELECT m.id, m.name, m.group_name, m.url, m.paused, s.status, s.last_latency_ms
             FROM monitors m JOIN monitor_state s ON s.monitor_id = m.id
             ORDER BY m.name_key",
        )
        .fetch_all(pool)
        .await
    }

    /// The group names in use, sorted, each spelled as on its first monitor by name, like on the
    /// dashboard: the form's suggestions.
    pub async fn group_names(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
        let mut names: Vec<String> = sqlx::query_scalar(
            "SELECT group_name FROM monitors WHERE group_name IS NOT NULL ORDER BY name_key",
        )
        .fetch_all(pool)
        .await?;
        // Stable: within a group, the first monitor's spelling stays first and is kept.
        names.sort_by_cached_key(|n| name_key(n));
        names.dedup_by(|a, b| name_key(a) == name_key(b));
        Ok(names)
    }

    /// The longest a failure series can run before reaching its threshold, over all monitors.
    pub async fn longest_failure_series_ms(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        let (ms,) = sqlx::query_as(
            "SELECT COALESCE(MAX(failure_threshold * (retry_interval_s + timeout_s)), 0) * 1000
             FROM monitors",
        )
        .fetch_one(pool)
        .await?;
        Ok(ms)
    }

    pub async fn find_all(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM monitors ORDER BY name_key"
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
                name, name_key, group_name, url, method, headers, body, keyword, accepted_statuses, follow_redirects,
                ignore_tls_errors, interval_s, retry_interval_s, timeout_s, failure_threshold,
                tls_expiry_warn_days, paused, created_at, updated_at, kind, grace_s, push_token
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&input.name)
        .bind(name_key(&input.name))
        .bind(&input.group_name)
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
        .bind(&input.kind)
        .bind(input.grace_s)
        .bind((input.kind == PUSH).then(random_token))
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();

        sqlx::query("INSERT INTO monitor_state (monitor_id) VALUES (?)")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        Channel::set_for_monitor(&mut tx, id, &input.channel_ids).await?;

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
            SET name = ?, name_key = ?, group_name = ?, url = ?, method = ?, headers = ?, body = ?, keyword = ?,
                accepted_statuses = ?, follow_redirects = ?, ignore_tls_errors = ?, interval_s = ?,
                retry_interval_s = ?, timeout_s = ?, failure_threshold = ?, tls_expiry_warn_days = ?, paused = ?, updated_at = ?,
                kind = ?, grace_s = ?,
                -- A push monitor keeps its URL across edits; a new one gets the token bound here.
                push_token = CASE WHEN ? = 'push' THEN COALESCE(push_token, ?) END
            WHERE id = ?
            "#,
        )
        .bind(&input.name)
        .bind(name_key(&input.name))
        .bind(&input.group_name)
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
        .bind(&input.kind)
        .bind(input.grace_s)
        // SET expressions read the old row: the new kind is bound again.
        .bind(&input.kind)
        .bind(random_token())
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }

        if input.paused {
            MonitorState::reset_for_pause(&mut tx, id, now).await?;
        }
        Channel::set_for_monitor(&mut tx, id, &input.channel_ids).await?;
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

    /// Pauses or resumes these monitors, as `toggle_pause` does each; unknown ids and monitors
    /// already so are skipped. Returns the ids changed.
    pub async fn set_paused(
        pool: &SqlitePool,
        ids: &[i64],
        paused: bool,
    ) -> Result<Vec<i64>, sqlx::Error> {
        let now = now_ms();
        let mut tx = pool.begin().await?;
        let changed: Vec<i64> = sqlx::query_scalar(
            "UPDATE monitors SET paused = ?, updated_at = ?
             WHERE id IN (SELECT value FROM json_each(?)) AND paused != ? RETURNING id",
        )
        .bind(paused)
        .bind(now)
        .bind(id_list(ids))
        .bind(paused)
        .fetch_all(&mut *tx)
        .await?;
        if paused {
            for &id in &changed {
                MonitorState::reset_for_pause(&mut tx, id, now).await?;
            }
        }
        tx.commit().await?;
        Ok(changed)
    }

    /// Returns the ids deleted; unknown ones are skipped.
    pub async fn delete_many(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar(
            "DELETE FROM monitors WHERE id IN (SELECT value FROM json_each(?)) RETURNING id",
        )
        .bind(id_list(ids))
        .fetch_all(pool)
        .await
    }

    /// Moves these monitors into a group (`None`: out of any), the name already cleaned. Returns
    /// how many changed group.
    pub async fn set_group(
        pool: &SqlitePool,
        ids: &[i64],
        group_name: Option<&str>,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE monitors SET group_name = ?, updated_at = ?
             WHERE id IN (SELECT value FROM json_each(?)) AND group_name IS NOT ?",
        )
        .bind(group_name)
        .bind(now_ms())
        .bind(id_list(ids))
        .bind(group_name)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
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

/// Ids as a JSON array, for `json_each`.
pub fn id_list(ids: &[i64]) -> String {
    serde_json::to_string(ids).expect("integers serialize")
}

/// The key that makes names unique and sorts them: `COLLATE NOCASE` only folds ASCII letters, and an accented letter
/// can be typed as one code point or as a letter plus a combining accent (NFC makes them equal).
pub fn name_key(name: &str) -> String {
    name.to_lowercase().nfc().collect()
}
