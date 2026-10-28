mod common;

use sqlx::SqlitePool;

async fn insert_monitor(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query(
        "INSERT INTO monitors (name, name_key, url, created_at, updated_at) VALUES (?1, ?1, 'https://example.com', 0, 0)",
    )
    .bind(name)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

#[tokio::test]
async fn settings_row_has_defaults() {
    let (_, pool) = common::test_app().await;
    let rows: Vec<(String, i64, Option<i64>, i64)> = sqlx::query_as(
        "SELECT timezone, checks_retention_days, history_retention_days, block_private_targets FROM settings",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![("UTC".to_string(), 7, None, 0)]);
}

#[tokio::test]
async fn settings_is_single_row() {
    let (_, pool) = common::test_app().await;
    let second = sqlx::query("INSERT INTO settings (id) VALUES (2)")
        .execute(&pool)
        .await;
    assert!(second.is_err());
}

#[tokio::test]
async fn only_one_open_incident_per_monitor() {
    let (_, pool) = common::test_app().await;
    let id = insert_monitor(&pool, "api").await;
    let open =
        "INSERT INTO incidents (monitor_id, started_at, error_kind) VALUES (?, 1, 'timeout')";
    sqlx::query(open).bind(id).execute(&pool).await.unwrap();
    assert!(sqlx::query(open).bind(id).execute(&pool).await.is_err());
    sqlx::query("INSERT INTO incidents (monitor_id, started_at, resolved_at, error_kind) VALUES (?, 2, 3, 'dns')")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn deleting_monitor_cascades() {
    let (_, pool) = common::test_app().await;
    let id = insert_monitor(&pool, "api").await;
    for sql in [
        "INSERT INTO monitor_state (monitor_id) VALUES (?)",
        "INSERT INTO checks (monitor_id, checked_at, ok) VALUES (?, 1, 1)",
        "INSERT INTO incidents (monitor_id, started_at, error_kind) VALUES (?, 1, 'timeout')",
        "INSERT INTO daily_stats (monitor_id, day, monitored_ms, downtime_ms, checks_total, checks_failed) VALUES (?, 20261028, 1, 0, 1, 0)",
    ] {
        sqlx::query(sql).bind(id).execute(&pool).await.unwrap();
    }
    sqlx::query("DELETE FROM monitors WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    for table in ["monitor_state", "checks", "incidents", "daily_stats"] {
        let (count,): (i64,) =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

#[tokio::test]
async fn check_constraints_hold() {
    let (_, pool) = common::test_app().await;
    let bad_method = sqlx::query(
        "INSERT INTO monitors (name, name_key, url, method, created_at, updated_at) VALUES ('a', 'a', 'u', 'TRACE', 0, 0)",
    )
    .execute(&pool)
    .await;
    assert!(bad_method.is_err());
    let bad_threshold = sqlx::query(
        "INSERT INTO monitors (name, name_key, url, failure_threshold, created_at, updated_at) VALUES ('b', 'b', 'u', 11, 0, 0)",
    )
    .execute(&pool)
    .await;
    assert!(bad_threshold.is_err());
    let id = insert_monitor(&pool, "c").await;
    let bad_status =
        sqlx::query("INSERT INTO monitor_state (monitor_id, status) VALUES (?, 'paused')")
            .bind(id)
            .execute(&pool)
            .await;
    assert!(bad_status.is_err());
}

#[tokio::test]
async fn check_engine_columns_have_defaults() {
    let (_, pool) = common::test_app().await;
    let id = insert_monitor(&pool, "api").await;
    sqlx::query("INSERT INTO monitor_state (monitor_id) VALUES (?)")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let row: (i64, i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT m.retry_interval_s, m.ignore_tls_errors, m.failure_threshold, s.failing_since
         FROM monitors m JOIN monitor_state s ON s.monitor_id = m.id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (20, 0, 3, None));
}
