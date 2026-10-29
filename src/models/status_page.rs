use crate::forms::status_page::{PageMonitorInput, StatusPageInput};
use crate::models::MonitorStatus;
use crate::time::now_ms;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

const COLUMNS: &str = "id, slug, title, description, published";

/// A public status page, served at `/status/{slug}` once published.
#[derive(Debug, Clone, FromRow)]
pub struct StatusPage {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub description: Option<String>,
    pub published: bool,
}

/// A row of the public pages list.
#[derive(Debug, Clone, FromRow)]
pub struct StatusPageSummary {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub published: bool,
    pub monitors: i64,
}

/// What a public page shows of a monitor: no URL, no error.
#[derive(Debug, Clone, FromRow)]
pub struct ShownMonitor {
    pub monitor_id: i64,
    /// The display name, or the monitor's own name.
    pub name: String,
    pub paused: bool,
    pub status: MonitorStatus,
}

impl StatusPage {
    pub async fn find(pool: &SqlitePool, id: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM status_pages WHERE id = ?"
        )))
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_slug(pool: &SqlitePool, slug: &str) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM status_pages WHERE slug = ?"
        )))
        .bind(slug)
        .fetch_optional(pool)
        .await
    }

    /// Sorted by title, with their monitor count.
    pub async fn summaries(pool: &SqlitePool) -> Result<Vec<StatusPageSummary>, sqlx::Error> {
        sqlx::query_as(
            "SELECT p.id, p.slug, p.title, p.published,
                    (SELECT count(*) FROM status_page_monitors WHERE page_id = p.id) AS monitors
             FROM status_pages p ORDER BY p.title COLLATE NOCASE, p.id",
        )
        .fetch_all(pool)
        .await
    }

    pub async fn create(pool: &SqlitePool, input: &StatusPageInput) -> Result<i64, sqlx::Error> {
        let now = now_ms();
        let mut tx = pool.begin().await?;
        let id = sqlx::query(
            "INSERT INTO status_pages (slug, title, description, published, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.slug)
        .bind(&input.title)
        .bind(&input.description)
        .bind(input.published)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        set_monitors(&mut tx, id, &input.monitors).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Returns `false` when no page has this id.
    pub async fn update(
        pool: &SqlitePool,
        id: i64,
        input: &StatusPageInput,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin().await?;
        let result = sqlx::query(
            "UPDATE status_pages SET slug = ?, title = ?, description = ?, published = ?,
                                     updated_at = ?
             WHERE id = ?",
        )
        .bind(&input.slug)
        .bind(&input.title)
        .bind(&input.description)
        .bind(input.published)
        .bind(now_ms())
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }
        set_monitors(&mut tx, id, &input.monitors).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Returns `false` when no page has this id.
    pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM status_pages WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// The page's monitors as stored, in display order.
    pub async fn monitors(
        pool: &SqlitePool,
        id: i64,
    ) -> Result<Vec<PageMonitorInput>, sqlx::Error> {
        let rows: Vec<(i64, Option<String>)> = sqlx::query_as(
            "SELECT monitor_id, display_name FROM status_page_monitors
             WHERE page_id = ? ORDER BY position",
        )
        .bind(id)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(monitor_id, display_name)| PageMonitorInput {
                monitor_id,
                display_name,
            })
            .collect())
    }

    /// The page's monitors with their state, in display order.
    pub async fn shown_monitors(
        pool: &SqlitePool,
        id: i64,
    ) -> Result<Vec<ShownMonitor>, sqlx::Error> {
        sqlx::query_as(
            "SELECT m.id AS monitor_id, COALESCE(pm.display_name, m.name) AS name, m.paused,
                    s.status
             FROM status_page_monitors pm
             JOIN monitors m ON m.id = pm.monitor_id
             JOIN monitor_state s ON s.monitor_id = m.id
             WHERE pm.page_id = ? ORDER BY pm.position",
        )
        .bind(id)
        .fetch_all(pool)
        .await
    }
}

/// Shows exactly these monitors, in this order; ids of monitors deleted meanwhile are ignored.
async fn set_monitors(
    conn: &mut SqliteConnection,
    page_id: i64,
    monitors: &[PageMonitorInput],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM status_page_monitors WHERE page_id = ?")
        .bind(page_id)
        .execute(&mut *conn)
        .await?;
    for (position, m) in monitors.iter().enumerate() {
        sqlx::query(
            "INSERT INTO status_page_monitors (page_id, monitor_id, display_name, position)
             SELECT ?, id, ?, ? FROM monitors WHERE id = ?",
        )
        .bind(page_id)
        .bind(&m.display_name)
        .bind(position as i64)
        .bind(m.monitor_id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
