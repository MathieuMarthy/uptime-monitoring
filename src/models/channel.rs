use crate::forms::channel::ChannelInput;
use crate::models::id_list;
use crate::notify::channel::ChannelConfig;
use crate::time::now_ms;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

const COLUMNS: &str = "id, name, kind, config, enabled";

/// A notification channel: where alerts of the monitors linked to it are sent.
#[derive(Debug, Clone, FromRow)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub config: String,
    pub enabled: bool,
}

/// A row of the alerts page.
#[derive(Debug, Clone, FromRow)]
pub struct ChannelSummary {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub monitors: i64,
    pub last_sent_at: Option<i64>,
    pub last_ok: Option<bool>,
}

impl Channel {
    /// Fails only if the stored JSON was edited by hand.
    pub fn parsed_config(&self) -> Result<ChannelConfig, serde_json::Error> {
        ChannelConfig::parse(&self.kind, &self.config)
    }

    pub async fn find(pool: &SqlitePool, id: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM notification_channels WHERE id = ?"
        )))
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    /// Sorted by name.
    pub async fn all(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM notification_channels ORDER BY name"
        )))
        .fetch_all(pool)
        .await
    }

    /// The enabled channels an alert of this monitor goes to.
    pub async fn enabled_for_monitor(
        pool: &SqlitePool,
        monitor_id: i64,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT c.id, c.name, c.kind, c.config, c.enabled
             FROM notification_channels c JOIN monitor_channels mc ON mc.channel_id = c.id
             WHERE mc.monitor_id = ? AND c.enabled = 1",
        )
        .bind(monitor_id)
        .fetch_all(pool)
        .await
    }

    /// Sorted by name, with their monitor count and last delivery.
    pub async fn summaries(pool: &SqlitePool) -> Result<Vec<ChannelSummary>, sqlx::Error> {
        sqlx::query_as(
            "SELECT c.id, c.name, c.kind, c.enabled,
                    (SELECT count(*) FROM monitor_channels WHERE channel_id = c.id) AS monitors,
                    l.sent_at AS last_sent_at, l.ok AS last_ok
             FROM notification_channels c
             LEFT JOIN notification_log l ON l.id =
                 (SELECT max(id) FROM notification_log WHERE channel_id = c.id)
             ORDER BY c.name",
        )
        .fetch_all(pool)
        .await
    }

    pub async fn create(pool: &SqlitePool, input: &ChannelInput) -> Result<i64, sqlx::Error> {
        let id = sqlx::query(
            "INSERT INTO notification_channels (name, kind, config, enabled, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&input.name)
        .bind(input.config.kind())
        .bind(input.config.to_json())
        .bind(input.enabled)
        .bind(now_ms())
        .execute(pool)
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    /// Returns `false` when no channel has this id.
    pub async fn update(
        pool: &SqlitePool,
        id: i64,
        input: &ChannelInput,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE notification_channels SET name = ?, kind = ?, config = ?, enabled = ?
             WHERE id = ?",
        )
        .bind(&input.name)
        .bind(input.config.kind())
        .bind(input.config.to_json())
        .bind(input.enabled)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Returns `false` when no channel has this id.
    pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM notification_channels WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Ids of the channels linked to this monitor.
    pub async fn ids_for_monitor(
        pool: &SqlitePool,
        monitor_id: i64,
    ) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar("SELECT channel_id FROM monitor_channels WHERE monitor_id = ?")
            .bind(monitor_id)
            .fetch_all(pool)
            .await
    }

    /// Every channel's id and name, by name.
    pub async fn names(pool: &SqlitePool) -> Result<Vec<(i64, String)>, sqlx::Error> {
        sqlx::query_as("SELECT id, name FROM notification_channels ORDER BY name")
            .fetch_all(pool)
            .await
    }

    /// Links the channel to these monitors; returns how many were not linked yet. Unknown ids,
    /// or a channel deleted meanwhile, link nothing.
    pub async fn link_monitors(
        pool: &SqlitePool,
        channel_id: i64,
        monitor_ids: &[i64],
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO monitor_channels (monitor_id, channel_id)
             SELECT m.id, c.id FROM monitors m, notification_channels c
             WHERE c.id = ? AND m.id IN (SELECT value FROM json_each(?))",
        )
        .bind(channel_id)
        .bind(id_list(monitor_ids))
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Returns how many of these monitors were linked to the channel.
    pub async fn unlink_monitors(
        pool: &SqlitePool,
        channel_id: i64,
        monitor_ids: &[i64],
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM monitor_channels
             WHERE channel_id = ? AND monitor_id IN (SELECT value FROM json_each(?))",
        )
        .bind(channel_id)
        .bind(id_list(monitor_ids))
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Links the monitor to exactly these channels; ids of channels deleted meanwhile are ignored.
    pub async fn set_for_monitor(
        conn: &mut SqliteConnection,
        monitor_id: i64,
        channel_ids: &[i64],
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM monitor_channels WHERE monitor_id = ?")
            .bind(monitor_id)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "INSERT INTO monitor_channels (monitor_id, channel_id)
             SELECT ?, id FROM notification_channels WHERE id IN (SELECT value FROM json_each(?))",
        )
        .bind(monitor_id)
        .bind(id_list(channel_ids))
        .execute(conn)
        .await?;
        Ok(())
    }
}
