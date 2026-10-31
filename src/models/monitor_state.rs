use crate::engine::slowness::{self, Sample, SlowTransition, Slowness};
use crate::engine::verdict::{self, CheckOutcome, Health, Transition};
use crate::models::{Check, Incident, Monitor, Slowdown};
use crate::time::DAY_MS;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

const COLUMNS: &str = "monitor_id, status, consecutive_failures, failing_since, last_check_at, last_latency_ms, \
    last_error, tls_expires_at, tls_warned_for, tls_warned_at, slow_checks, slow_since, degraded_since";

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
    pub slow_transition: Option<SlowTransition>,
    /// `slow_transition` is to be alerted: a start with alerts on, the end of a start alerted.
    pub slow_alert: bool,
}

/// The settings of a monitor that drive its state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub failure_threshold: i64,
    /// `None`: no certificate warning.
    pub tls_warn_days: Option<i64>,
    /// `None`: no degraded state.
    pub latency_warn_ms: Option<i64>,
    pub latency_alerts: bool,
}

impl Rules {
    pub fn of(monitor: &Monitor) -> Self {
        Self {
            failure_threshold: monitor.failure_threshold,
            tls_warn_days: monitor.tls_expiry_warn_days,
            latency_warn_ms: monitor.latency_warn_ms,
            latency_alerts: monitor.latency_alerts,
        }
    }
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
    /// When the warning about `tls_warned_for` was given.
    pub tls_warned_at: Option<i64>,
    pub slow_checks: i64,
    pub slow_since: Option<i64>,
    /// Since when the monitor is degraded; `None` when it is not.
    pub degraded_since: Option<i64>,
}

/// A certificate warning, for the Activity page.
#[derive(Debug, Clone, FromRow)]
pub struct CertificateWarning {
    pub monitor_id: i64,
    pub monitor_name: String,
    pub warned_at: i64,
    /// The expiry warned about.
    pub expires_at: i64,
    /// The certificate seen by the last check: a later date means it was renewed.
    pub current_expiry: Option<i64>,
}

impl MonitorState {
    /// Up to `limit` warnings given from `from` to `before`, latest first; those at `before`
    /// itself are kept when their monitor id is below `tie_id`. Only the last warning of each
    /// monitor is kept.
    pub async fn warnings_between(
        pool: &SqlitePool,
        monitor_id: Option<i64>,
        from: i64,
        before: i64,
        tie_id: i64,
        limit: i64,
    ) -> Result<Vec<CertificateWarning>, sqlx::Error> {
        sqlx::query_as(
            "SELECT s.monitor_id, m.name AS monitor_name, s.tls_warned_at AS warned_at,
                    s.tls_warned_for AS expires_at, s.tls_expires_at AS current_expiry
             FROM monitor_state s JOIN monitors m ON m.id = s.monitor_id
             WHERE s.tls_warned_at >= ?1
               AND (s.tls_warned_at < ?2 OR (s.tls_warned_at = ?2 AND s.monitor_id < ?5))
               AND s.tls_warned_for IS NOT NULL AND (?3 IS NULL OR s.monitor_id = ?3)
             ORDER BY s.tls_warned_at DESC, s.monitor_id DESC LIMIT ?4",
        )
        .bind(from)
        .bind(before)
        .bind(monitor_id)
        .bind(limit)
        .bind(tie_id)
        .fetch_all(pool)
        .await
    }

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
    ///
    /// During a maintenance window (`in_maintenance`) the check is kept but covers no time, and
    /// the state machine is frozen: no transition, no certificate warning, and a failure series
    /// below the threshold starts again after the window. A success still ends an outage, but
    /// raises no alert.
    ///
    /// Slow responses open and resolve slowdowns (see `slowness::apply`).
    pub async fn record(
        pool: &SqlitePool,
        monitor_id: i64,
        outcome: &CheckOutcome,
        health: Health,
        rules: &Rules,
        in_maintenance: bool,
    ) -> Result<Option<Recorded>, sqlx::Error> {
        let (next, transition) = if in_maintenance {
            (verdict::frozen(health, outcome.ok()), None)
        } else {
            verdict::apply(
                health,
                outcome.ok(),
                outcome.checked_at,
                rules.failure_threshold,
            )
        };
        let tls_warn_days = rules.tls_warn_days.filter(|_| !in_maintenance);
        let mut tx = pool.begin().await?;

        // Written first so the transaction takes the write lock at once. RETURNING gives the
        // slowness as it was: this statement leaves it alone.
        let before: Option<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
            "UPDATE monitor_state
             SET status = ?, consecutive_failures = ?, failing_since = ?, last_check_at = ?,
                 last_latency_ms = ?, last_error = ?,
                 tls_expires_at = CASE WHEN ? THEN ? ELSE tls_expires_at END
             WHERE monitor_id = ? AND (SELECT paused FROM monitors WHERE id = ?) = 0
               AND status = ? AND consecutive_failures = ? AND failing_since IS ?
             RETURNING slow_checks, slow_since, degraded_since",
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
        .fetch_optional(&mut *tx)
        .await?;
        let Some((slow_checks, slow_since, degraded_since)) = before else {
            return Ok(None);
        };

        Check::insert(&mut tx, monitor_id, outcome, in_maintenance).await?;
        match transition {
            Some(Transition::WentDown { since }) => {
                let latest = outcome.failure.as_ref().expect("only a failure goes down");
                Incident::open(&mut tx, monitor_id, since, latest).await?
            }
            Some(Transition::Recovered) => {
                Incident::resolve(&mut tx, monitor_id, outcome.checked_at).await?
            }
            // A success during a window: the outage is over, silently.
            None if health.status == MonitorStatus::Down && next.status == MonitorStatus::Up => {
                Incident::resolve(&mut tx, monitor_id, outcome.checked_at).await?
            }
            None => {}
        }

        let slow_before = Slowness {
            slow_checks,
            slow_since,
            degraded_since,
        };
        let (slow_after, slow_transition) = slowness::apply(
            slow_before,
            Sample {
                checked_at: outcome.checked_at,
                latency_ms: outcome.latency_ms.filter(|_| outcome.ok()),
            },
            rules.latency_warn_ms,
            rules.failure_threshold,
            matches!(transition, Some(Transition::WentDown { .. })),
            in_maintenance,
        );
        if slow_after != slow_before {
            sqlx::query(
                "UPDATE monitor_state SET slow_checks = ?, slow_since = ?, degraded_since = ?
                 WHERE monitor_id = ?",
            )
            .bind(slow_after.slow_checks)
            .bind(slow_after.slow_since)
            .bind(slow_after.degraded_since)
            .bind(monitor_id)
            .execute(&mut *tx)
            .await?;
        }
        let slow_alert = match slow_transition {
            Some(SlowTransition::Degraded {
                since,
                threshold_ms,
            }) => {
                Slowdown::open(
                    &mut tx,
                    monitor_id,
                    since,
                    threshold_ms,
                    rules.latency_alerts,
                )
                .await?;
                rules.latency_alerts
            }
            Some(SlowTransition::Normal { .. }) => {
                Slowdown::resolve(&mut tx, monitor_id, outcome.checked_at).await?
            }
            // Ended silently: where the outage starts, or at this check.
            None if slow_before.degraded_since.is_some() && slow_after.degraded_since.is_none() => {
                let at = match transition {
                    Some(Transition::WentDown { since }) => since,
                    _ => outcome.checked_at,
                };
                Slowdown::resolve(&mut tx, monitor_id, at).await?;
                false
            }
            None => false,
        };

        let tls_warning =
            match tls_warn_days {
                Some(days) => sqlx::query_scalar(
                    "UPDATE monitor_state SET tls_warned_for = tls_expires_at, tls_warned_at = ?
                     WHERE monitor_id = ? AND tls_expires_at < ?
                       AND tls_warned_for IS NOT tls_expires_at
                     RETURNING tls_expires_at",
                )
                .bind(outcome.checked_at)
                .bind(monitor_id)
                .bind(outcome.checked_at + days * DAY_MS)
                .fetch_optional(&mut *tx)
                .await?,
                None => None,
            };

        tx.commit().await?;
        Ok(Some(Recorded {
            health: next,
            transition,
            tls_warning,
            slow_transition,
            slow_alert,
        }))
    }

    /// Drops the slow series and ends the slowdown at `at`, silently: the threshold changed, so
    /// what was measured against the old one no longer holds.
    pub async fn clear_slowness(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        at: i64,
    ) -> Result<(), sqlx::Error> {
        Slowdown::resolve(&mut *conn, monitor_id, at).await?;
        sqlx::query(
            "UPDATE monitor_state SET slow_checks = 0, slow_since = NULL, degraded_since = NULL
             WHERE monitor_id = ?",
        )
        .bind(monitor_id)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// On pause: resolves the open incident and slowdown at the last check (nothing is known after it, so ending later would count
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
        Slowdown::resolve(&mut *conn, monitor_id, end).await?;
        sqlx::query(
            "UPDATE monitor_state
             SET status = 'pending', consecutive_failures = 0, failing_since = NULL,
                 slow_checks = 0, slow_since = NULL, degraded_since = NULL
             WHERE monitor_id = ?",
        )
        .bind(monitor_id)
        .execute(conn)
        .await?;
        Ok(())
    }
}
