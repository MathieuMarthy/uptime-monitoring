use crate::models::id_list;
use crate::time::now_ms;
use crate::windows::Schedule;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

const COLUMNS: &str = "id, title, all_monitors, show_on_status_pages, starts_at, ends_at, weekdays, \
    local_start, duration_min, created_at";

/// A maintenance window: its monitors' checks run but neither count nor alert while it is on.
#[derive(Debug, Clone, FromRow)]
pub struct Maintenance {
    pub id: i64,
    pub title: String,
    /// Every monitor, present and future; the linked monitors are then ignored.
    pub all_monitors: bool,
    /// Announced on the status pages showing one of its monitors, up to 7 days ahead.
    pub show_on_status_pages: bool,
    starts_at: Option<i64>,
    ends_at: Option<i64>,
    weekdays: Option<i64>,
    local_start: Option<i64>,
    duration_min: Option<i64>,
    /// Occurrences before it never happened.
    pub created_at: i64,
}

/// A validated maintenance, ready to be stored.
#[derive(Debug, Clone, PartialEq)]
pub struct MaintenanceInput {
    pub title: String,
    pub schedule: Schedule,
    pub all_monitors: bool,
    pub show_on_status_pages: bool,
    pub monitor_ids: Vec<i64>,
}

impl Maintenance {
    pub fn schedule(&self) -> Schedule {
        match (self.starts_at, self.ends_at) {
            (Some(starts_at), Some(ends_at)) => Schedule::Once { starts_at, ends_at },
            // The schema allows nothing else.
            _ => Schedule::Weekly {
                weekdays: self.weekdays.unwrap_or(0) as u8,
                local_start: self.local_start.unwrap_or(0),
                duration_min: self.duration_min.unwrap_or(0),
            },
        }
    }

    /// Every maintenance with its monitors, by id.
    pub async fn all(pool: &SqlitePool) -> Result<Vec<(Self, Vec<i64>)>, sqlx::Error> {
        let maintenances: Vec<Self> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM maintenances ORDER BY id"
        )))
        .fetch_all(pool)
        .await?;
        let links: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT maintenance_id, monitor_id FROM maintenance_monitors
             ORDER BY maintenance_id, monitor_id",
        )
        .fetch_all(pool)
        .await?;
        Ok(maintenances
            .into_iter()
            .map(|m| {
                let ids = links
                    .iter()
                    .filter(|(id, _)| *id == m.id)
                    .map(|(_, monitor)| *monitor)
                    .collect();
                (m, ids)
            })
            .collect())
    }

    pub async fn find(pool: &SqlitePool, id: i64) -> Result<Option<(Self, Vec<i64>)>, sqlx::Error> {
        let Some(found) = sqlx::query_as::<_, Self>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM maintenances WHERE id = ?"
        )))
        .bind(id)
        .fetch_optional(pool)
        .await?
        else {
            return Ok(None);
        };
        let ids = sqlx::query_scalar(
            "SELECT monitor_id FROM maintenance_monitors WHERE maintenance_id = ? ORDER BY monitor_id",
        )
        .bind(id)
        .fetch_all(pool)
        .await?;
        Ok(Some((found, ids)))
    }

    pub async fn create(pool: &SqlitePool, input: &MaintenanceInput) -> Result<i64, sqlx::Error> {
        let now = now_ms();
        let (once, weekly) = split(&input.schedule);
        let mut tx = pool.begin().await?;
        let id = sqlx::query(
            "INSERT INTO maintenances (title, all_monitors, show_on_status_pages, starts_at, ends_at,
                 weekdays, local_start, duration_min, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.title)
        .bind(input.all_monitors)
        .bind(input.show_on_status_pages)
        .bind(once.map(|o| o.0))
        .bind(once.map(|o| o.1))
        .bind(weekly.map(|w| w.0))
        .bind(weekly.map(|w| w.1))
        .bind(weekly.map(|w| w.2))
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        set_monitors(&mut tx, id, &input.monitor_ids).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Returns `false` when no maintenance has this id.
    pub async fn update(
        pool: &SqlitePool,
        id: i64,
        input: &MaintenanceInput,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin().await?;
        if !write(&mut tx, id, input, now_ms()).await? {
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Returns `false` when no maintenance has this id.
    pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM maintenances WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Deletes the one-off windows ended before `cutoff`, with the history.
    pub async fn purge_ended_before(pool: &SqlitePool, cutoff: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM maintenances WHERE ends_at < ?")
            .bind(cutoff)
            .execute(pool)
            .await?;
        Ok(())
    }
}

type OnceColumns = (i64, i64);
type WeeklyColumns = (i64, i64, i64);

/// A schedule as its columns: `(starts_at, ends_at)` or `(weekdays, local_start, duration_min)`.
fn split(schedule: &Schedule) -> (Option<OnceColumns>, Option<WeeklyColumns>) {
    match *schedule {
        Schedule::Once { starts_at, ends_at } => (Some((starts_at, ends_at)), None),
        Schedule::Weekly {
            weekdays,
            local_start,
            duration_min,
        } => (None, Some((i64::from(weekdays), local_start, duration_min))),
    }
}

/// Writes the fields and monitors of maintenance `id`. Returns `false` when it does not exist.
async fn write(
    conn: &mut SqliteConnection,
    id: i64,
    input: &MaintenanceInput,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let (once, weekly) = split(&input.schedule);
    let updated = sqlx::query(
        "UPDATE maintenances SET title = ?, all_monitors = ?, show_on_status_pages = ?,
             starts_at = ?, ends_at = ?, weekdays = ?, local_start = ?, duration_min = ?,
             updated_at = ?
         WHERE id = ?",
    )
    .bind(&input.title)
    .bind(input.all_monitors)
    .bind(input.show_on_status_pages)
    .bind(once.map(|o| o.0))
    .bind(once.map(|o| o.1))
    .bind(weekly.map(|w| w.0))
    .bind(weekly.map(|w| w.1))
    .bind(weekly.map(|w| w.2))
    .bind(now)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(false);
    }
    set_monitors(conn, id, &input.monitor_ids).await?;
    Ok(true)
}

/// Replaces the monitors of maintenance `id`; monitors deleted meanwhile are skipped.
async fn set_monitors(
    conn: &mut SqliteConnection,
    id: i64,
    monitor_ids: &[i64],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM maintenance_monitors WHERE maintenance_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO maintenance_monitors (maintenance_id, monitor_id)
         SELECT ?, id FROM monitors WHERE id IN (SELECT value FROM json_each(?))",
    )
    .bind(id)
    .bind(id_list(monitor_ids))
    .execute(conn)
    .await?;
    Ok(())
}
