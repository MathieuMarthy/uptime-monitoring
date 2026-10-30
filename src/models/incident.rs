use crate::engine::verdict::Failure;
use crate::models::id_list;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

/// An outage period: from the first failed check of the series to the first successful check.
#[derive(Debug, Clone, FromRow)]
pub struct Incident {
    pub id: i64,
    pub monitor_id: i64,
    pub started_at: i64,
    pub resolved_at: Option<i64>,
    pub error_kind: String,
    pub cause: Option<String>,
    /// Written by the admin.
    pub note: Option<String>,
    /// The note also shows on status pages.
    pub note_public: bool,
}

/// An incident with the name of its monitor and the bounds of the next incident on that monitor.
#[derive(Debug, Clone, FromRow)]
pub struct NamedIncident {
    #[sqlx(flatten)]
    pub incident: Incident,
    pub monitor_name: String,
    pub next_started_at: Option<i64>,
    pub next_resolved_at: Option<i64>,
}

impl Incident {
    /// Up to `limit` incidents started from `from` to `before`, latest first, for one monitor or
    /// all of them. Those started at `before` itself are kept when their id is below `tie_id`.
    pub async fn started_between(
        pool: &SqlitePool,
        monitor_id: Option<i64>,
        from: i64,
        before: i64,
        tie_id: i64,
        limit: i64,
    ) -> Result<Vec<NamedIncident>, sqlx::Error> {
        sqlx::query_as(
            "SELECT i.id, i.monitor_id, i.started_at, i.resolved_at, i.error_kind, i.cause, i.note, i.note_public,
                    m.name AS monitor_name, n.started_at AS next_started_at,
                    n.resolved_at AS next_resolved_at
             FROM incidents i JOIN monitors m ON m.id = i.monitor_id
             LEFT JOIN incidents n ON n.id = (SELECT id FROM incidents
                                              WHERE monitor_id = i.monitor_id
                                                AND started_at > i.started_at
                                              ORDER BY started_at LIMIT 1)
             WHERE i.started_at >= ?1 AND (i.started_at < ?2 OR (i.started_at = ?2 AND i.id < ?5))
               AND (?3 IS NULL OR i.monitor_id = ?3)
             ORDER BY i.started_at DESC, i.id DESC LIMIT ?4",
        )
        .bind(from)
        .bind(before)
        .bind(monitor_id)
        .bind(limit)
        .bind(tie_id)
        .fetch_all(pool)
        .await
    }

    /// Most recent first.
    pub async fn for_monitor(pool: &SqlitePool, monitor_id: i64) -> Result<Vec<Self>, sqlx::Error> {
        Self::recent(pool, monitor_id, -1).await
    }

    /// The `limit` most recent, first; a negative limit returns them all.
    pub async fn recent(
        pool: &SqlitePool,
        monitor_id: i64,
        limit: i64,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, monitor_id, started_at, resolved_at, error_kind, cause, note, note_public FROM incidents
             WHERE monitor_id = ? ORDER BY started_at DESC LIMIT ?",
        )
        .bind(monitor_id)
        .bind(limit)
        .fetch_all(pool)
        .await
    }

    /// Up to `limit` incidents of these monitors that were open at some point since `since`: those
    /// still open first, then the most recent.
    pub async fn recent_for_monitors(
        pool: &SqlitePool,
        monitor_ids: &[i64],
        since: i64,
        limit: i64,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, monitor_id, started_at, resolved_at, error_kind, cause, note, note_public FROM incidents
             WHERE monitor_id IN (SELECT value FROM json_each(?))
               AND (resolved_at IS NULL OR resolved_at >= ?)
             ORDER BY resolved_at IS NULL DESC, started_at DESC LIMIT ?",
        )
        .bind(id_list(monitor_ids))
        .bind(since)
        .bind(limit)
        .fetch_all(pool)
        .await
    }

    /// Sets the note (`None` clears it) and whether it is public. Returns the incident's monitor, or
    /// `None` when no incident has this id.
    pub async fn set_note(
        pool: &SqlitePool,
        id: i64,
        note: Option<&str>,
        public: bool,
    ) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar(
            "UPDATE incidents SET note = ?, note_public = ? WHERE id = ? RETURNING monitor_id",
        )
        .bind(note)
        .bind(public)
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    /// Opens an incident starting at the failed check recorded at `since`, with its error, and
    /// marks the checks after it as downtime. If that check was already purged, `latest` (the
    /// failure that reached the threshold) gives the error instead.
    pub async fn open(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        since: i64,
        latest: &Failure,
    ) -> Result<(), sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO incidents (monitor_id, started_at, error_kind, cause)
             SELECT monitor_id, checked_at, error_kind, error FROM checks
             WHERE monitor_id = ? AND checked_at = ?",
        )
        .bind(monitor_id)
        .bind(since)
        .execute(&mut *conn)
        .await?;
        if inserted.rows_affected() == 0 {
            sqlx::query(
                "INSERT INTO incidents (monitor_id, started_at, error_kind, cause) VALUES (?, ?, ?, ?)",
            )
            .bind(monitor_id)
            .bind(since)
            .bind(latest.kind.as_str())
            .bind(&latest.message)
            .execute(&mut *conn)
            .await?;
        }
        // The failures before the threshold was reached were downtime too.
        sqlx::query("UPDATE checks SET down = 1 WHERE monitor_id = ? AND checked_at > ?")
            .bind(monitor_id)
            .bind(since)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Resolves the open incident, if any.
    pub async fn resolve(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        at: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE incidents SET resolved_at = ? WHERE monitor_id = ? AND resolved_at IS NULL",
        )
        .bind(at)
        .bind(monitor_id)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Deletes the incidents resolved before `cutoff`; an open incident stays, however old.
    pub async fn purge_resolved_before(pool: &SqlitePool, cutoff: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM incidents WHERE resolved_at < ?")
            .bind(cutoff)
            .execute(pool)
            .await?;
        Ok(())
    }
}
