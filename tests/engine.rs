mod common;

use axum::Router;
use axum::routing::get;
use common::post_form;
use sqlx::SqlitePool;
use std::time::Duration;
use uptime_monitoring::engine;
use uptime_monitoring::engine::guard::Guard;
use uptime_monitoring::forms::monitor::MonitorForm;
use uptime_monitoring::models::{Monitor, MonitorState, MonitorStatus};
use uptime_monitoring::notify::Notifier;

async fn target() -> String {
    let app = Router::new().route("/ok", get(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/ok")
}

async fn start(pool: &SqlitePool) -> engine::EngineHandle {
    engine::start(pool.clone(), Guard::new(false), Notifier::default())
        .await
        .unwrap()
}

async fn create(pool: &SqlitePool, url: &str) -> i64 {
    let input = MonitorForm {
        // Each target listens on its own port, so the URL is a unique name.
        name: url.to_string(),
        url: url.to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap();
    Monitor::create(pool, &input).await.unwrap()
}

async fn checks(pool: &SqlitePool, id: i64) -> i64 {
    sqlx::query_as::<_, (i64,)>("SELECT count(*) FROM checks WHERE monitor_id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
        .0
}

/// Waits up to 5 s for the monitor to have `n` checks.
async fn wait_for_checks(pool: &SqlitePool, id: i64, n: i64) {
    for _ in 0..100 {
        if checks(pool, id).await >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("monitor {id} never reached {n} checks");
}

#[tokio::test]
async fn reload_checks_a_new_monitor_at_once() {
    let pool = common::test_app().await.1;
    let handle = start(&pool).await;
    let id = create(&pool, &target().await).await;
    handle.reload(id);
    wait_for_checks(&pool, id, 1).await;
    let state = MonitorState::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(state.status, MonitorStatus::Up);
}

#[tokio::test]
async fn reload_after_edit_checks_again_at_once() {
    let pool = common::test_app().await.1;
    let handle = start(&pool).await;
    let id = create(&pool, &target().await).await;
    handle.reload(id);
    wait_for_checks(&pool, id, 1).await;
    handle.reload(id);
    wait_for_checks(&pool, id, 2).await;
}

#[tokio::test]
async fn paused_monitor_is_not_checked() {
    let pool = common::test_app().await.1;
    let handle = start(&pool).await;
    let id = create(&pool, &target().await).await;
    Monitor::toggle_pause(&pool, id).await.unwrap();
    handle.reload(id);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(checks(&pool, id).await, 0);

    Monitor::toggle_pause(&pool, id).await.unwrap();
    handle.reload(id);
    wait_for_checks(&pool, id, 1).await;
}

#[tokio::test]
async fn deleted_monitor_stops_being_checked() {
    let pool = common::test_app().await.1;
    let handle = start(&pool).await;
    let id = create(&pool, &target().await).await;
    // Created first: SQLite would reuse the deleted id.
    let other = create(&pool, &target().await).await;
    sqlx::query("UPDATE monitors SET paused = 1 WHERE id = ?")
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    handle.reload(id);
    wait_for_checks(&pool, id, 1).await;
    Monitor::delete(&pool, id).await.unwrap();
    handle.reload(id);
    // Reloads are handled in order: once `other` is checked, `id`'s task is gone.
    sqlx::query("UPDATE monitors SET paused = 0 WHERE id = ?")
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    handle.reload(other);
    wait_for_checks(&pool, other, 1).await;
    assert_eq!(
        checks(&pool, id).await,
        0,
        "cascade removed the rows, none came back"
    );
}

#[tokio::test]
async fn disconnected_handle_ignores_reloads() {
    engine::EngineHandle::default().reload(1);
}

#[tokio::test]
async fn monitor_routes_drive_the_engine() {
    let (app, pool) = common::test_app_with_engine().await;
    let url = target().await;
    let form = format!(
        "name=api&url={}&method=GET&accepted_statuses=200-299&interval_s=60\
         &retry_interval_s=20&timeout_s=10&failure_threshold=3&tls_expiry_warn_days=14\
         &follow_redirects=on&active=on",
        url.replace(':', "%3A").replace('/', "%2F")
    );
    assert_eq!(post_form(&app, "/monitors/new", &form).await.status, 303);
    wait_for_checks(&pool, 1, 1).await;

    post_form(&app, "/monitors/1/edit", &form).await;
    wait_for_checks(&pool, 1, 2).await;

    post_form(&app, "/monitors/1/toggle-pause", "").await;
    post_form(&app, "/monitors/1/toggle-pause", "").await;
    wait_for_checks(&pool, 1, 3).await;
}
