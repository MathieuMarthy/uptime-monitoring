use crate::engine::verdict::{self, CheckOutcome, Health, Transition};
use crate::models::{Check, Incident};
use crate::time::DAY_MS;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

const COLUMNS: &str = "monitor_id, status, consecutive_failures, failing_since, last_check_at, last_latency_ms, \
    last_error, tls_expires_at, tls_warned_for";

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum MonitorStatus {
    Pending,
    Up,
    Down,
}

/// What recording a check changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recorded {
    pub health: Health,
    pub transition: Option<Transition>,
    /// The certificate expiry to warn about, the first time it comes within the warning period.
    pub tls_warning: Option<i64>,
}

/// A monitor's runtime state, written only by the check engine.
#[derive(Debug, Clone, FromRow)]
pub struct MonitorState {
    pub monitor_id: i64,
    pub status: MonitorStatus,
    pub consecutive_failures: i64,
    pub failing_since: Option<i64>,
    pub last_check_at: Option<i64>,
    pub last_latency_ms: Option<i64>,
    pub last_error: Option<String>,
    pub tls_expires_at: Option<i64>,
    pub tls_warned_for: Option<i64>,
}

impl MonitorState {
    pub async fn find(pool: &SqlitePool, monitor_id: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM monitor_state WHERE monitor_id = ?"
        )))
        .bind(monitor_id)
        .fetch_optional(pool)
        .await
    }

    pub fn health(&self) -> Health {
        Health {
            status: self.status,
            consecutive_failures: self.consecutive_failures,
            failing_since: self.failing_since,
        }
    }

    /// Records a check in one transaction: state, raw check, the incident it opens or resolves,
    /// and the certificate warning it triggers (once per expiry date, when the known certificate
    /// expires within `tls_warn_days`). `health` is the state the caller last knew. Returns
    /// `None`, recording nothing, when the monitor is paused or deleted, or when its state no
    /// longer matches `health` (a pause reset it meanwhile): the caller must stop and let a
    /// fresh task take over.
    pub async fn record(
        pool: &SqlitePool,
        monitor_id: i64,
        outcome: &CheckOutcome,
        health: Health,
        failure_threshold: i64,
        tls_warn_days: Option<i64>,
    ) -> Result<Option<Recorded>, sqlx::Error> {
        let (next, transition) =
            verdict::apply(health, outcome.ok(), outcome.checked_at, failure_threshold);
        let mut tx = pool.begin().await?;

        // Written first so the transaction takes the write lock at once.
        let updated = sqlx::query(
            "UPDATE monitor_state
             SET status = ?, consecutive_failures = ?, failing_since = ?, last_check_at = ?,
                 last_latency_ms = ?, last_error = ?,
                 tls_expires_at = CASE WHEN ? THEN ? ELSE tls_expires_at END
             WHERE monitor_id = ? AND (SELECT paused FROM monitors WHERE id = ?) = 0
               AND status = ? AND consecutive_failures = ? AND failing_since IS ?",
        )
        .bind(next.status)
        .bind(next.consecutive_failures)
        .bind(next.failing_since)
        .bind(outcome.checked_at)
        .bind(outcome.latency_ms)
        .bind(outcome.failure.as_ref().map(|f| f.message.as_str()))
        // A response proves which certificate (if any) is in use; without one, keep the last.
        .bind(outcome.status_code.is_some())
        .bind(outcome.tls_expires_at)
        .bind(monitor_id)
        .bind(monitor_id)
        .bind(health.status)
        .bind(health.consecutive_failures)
        .bind(health.failing_since)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }

        Check::insert(&mut tx, monitor_id, outcome).await?;
        match transition {
            Some(Transition::WentDown { since }) => {
                let latest = outcome.failure.as_ref().expect("only a failure goes down");
                Incident::open(&mut tx, monitor_id, since, latest).await?
            }
            Some(Transition::Recovered) => {
                Incident::resolve(&mut tx, monitor_id, outcome.checked_at).await?
            }
            None => {}
        }

        let tls_warning = match tls_warn_days {
            Some(days) => {
                sqlx::query_scalar(
                    "UPDATE monitor_state SET tls_warned_for = tls_expires_at
                     WHERE monitor_id = ? AND tls_expires_at < ?
                       AND tls_warned_for IS NOT tls_expires_at
                     RETURNING tls_expires_at",
                )
                .bind(monitor_id)
                .bind(outcome.checked_at + days * DAY_MS)
                .fetch_optional(&mut *tx)
                .await?
            }
            None => None,
        };

        tx.commit().await?;
        Ok(Some(Recorded {
            health: next,
            transition,
            tls_warning,
        }))
    }

    /// On pause: resolves the open incident at the last check (nothing is known after it, so ending later would count
    /// unseen time as an outage) and puts the monitor back to `pending`.
    pub async fn reset_for_pause(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        now: i64,
    ) -> Result<(), sqlx::Error> {
        let last_check: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT last_check_at FROM monitor_state WHERE monitor_id = ?")
                .bind(monitor_id)
                .fetch_optional(&mut *conn)
                .await?;
        let end = last_check.and_then(|(at,)| at).unwrap_or(now);
        Incident::resolve(&mut *conn, monitor_id, end).await?;
        sqlx::query(
            "UPDATE monitor_state
             SET status = 'pending', consecutive_failures = 0, failing_since = NULL
             WHERE monitor_id = ?",
        )
        .bind(monitor_id)
        .execute(conn)
        .await?;
        Ok(())
    }
}
