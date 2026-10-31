mod common;

use axum::body::Body;
use axum::http::{Method, Request};
use common::{TestApp, get, post_form, send};
use sqlx::SqlitePool;
use std::time::Duration;
use uptime_monitoring::models::{MonitorState, MonitorStatus};

const PUSH: &str = "name=backup&kind=push&interval_s=60&grace_s=60&retry_interval_s=20\
    &failure_threshold=1&active=on";

async fn create(app: &TestApp, form: &str) -> i64 {
    let r = post_form(app, "/monitors/new", form).await;
    assert_eq!(r.status, 303, "body: {}", r.body);
    r.headers["location"]
        .to_str()
        .unwrap()
        .trim_start_matches("/monitors/")
        .parse()
        .unwrap()
}

async fn token(pool: &SqlitePool, id: i64) -> Option<String> {
    sqlx::query_as::<_, (Option<String>,)>("SELECT push_token FROM monitors WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
        .0
}

/// `(ok, error_kind, error)` of every check, oldest first.
async fn checks(pool: &SqlitePool, id: i64) -> Vec<(bool, Option<String>, Option<String>)> {
    sqlx::query_as(
        "SELECT ok, error_kind, error FROM checks WHERE monitor_id = ? ORDER BY checked_at",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Waits up to `seconds` for the monitor to have `n` checks.
async fn wait_for_checks(pool: &SqlitePool, id: i64, n: usize, seconds: u64) {
    for _ in 0..seconds * 20 {
        if checks(pool, id).await.len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("monitor {id} never reached {n} checks");
}

async fn status(pool: &SqlitePool, id: i64) -> MonitorStatus {
    MonitorState::find(pool, id).await.unwrap().unwrap().status
}

async fn ping(app: &TestApp, path: &str) -> u16 {
    get(&app.logged_out(), path).await.status.as_u16()
}

#[tokio::test]
async fn unknown_token_is_not_found() {
    let (app, _) = common::test_app().await;
    assert_eq!(ping(&app, "/push/abc").await, 404);
}

#[tokio::test]
async fn push_monitor_gets_a_token_shown_on_its_page() {
    let (app, pool) = common::test_app().await;
    let id = create(&app, PUSH).await;
    let token = token(&pool, id).await.unwrap();
    assert_eq!(token.len(), 64);
    let r = get(&app, &format!("/monitors/{id}")).await;
    assert!(r.body.contains(&format!("/push/{token}")), "{}", r.body);
}

#[tokio::test]
async fn http_monitor_has_no_token() {
    let (app, pool) = common::test_app().await;
    let form = "name=api&kind=http&url=https%3A%2F%2Fexample.com&method=GET\
        &accepted_statuses=200-299&interval_s=60&retry_interval_s=20&timeout_s=10\
        &failure_threshold=1&active=on";
    let id = create(&app, form).await;
    assert_eq!(token(&pool, id).await, None);
}

#[tokio::test]
async fn ping_records_a_success() {
    let (app, pool) = common::test_app_with_engine().await;
    let id = create(&app, PUSH).await;
    let token = token(&pool, id).await.unwrap();

    let r = get(&app.logged_out(), &format!("/push/{token}")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, "ok");
    wait_for_checks(&pool, id, 1, 5).await;
    assert_eq!(checks(&pool, id).await, vec![(true, None, None)]);
    assert_eq!(status(&pool, id).await, MonitorStatus::Up);
}

#[tokio::test]
async fn reported_failure_is_recorded_with_its_message() {
    let (app, pool) = common::test_app_with_engine().await;
    let id = create(&app, PUSH).await;
    let token = token(&pool, id).await.unwrap();

    let path = format!("/push/{token}?status=down&msg=disk%20full");
    assert_eq!(ping(&app, &path).await, 200);
    wait_for_checks(&pool, id, 1, 5).await;
    assert_eq!(
        checks(&pool, id).await,
        vec![(
            false,
            Some("reported".to_string()),
            Some("disk full".to_string())
        )]
    );
    assert_eq!(status(&pool, id).await, MonitorStatus::Down);
}

#[tokio::test]
async fn head_and_post_are_accepted_and_a_burst_records_once() {
    let (app, pool) = common::test_app_with_engine().await;
    let id = create(&app, PUSH).await;
    let path = format!("/push/{}", token(&pool, id).await.unwrap());

    assert_eq!(ping(&app, &path).await, 200);
    let head = Request::builder()
        .method(Method::HEAD)
        .uri(&path)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app.logged_out(), head).await.status, 200);
    assert_eq!(post_form(&app.logged_out(), &path, "").await.status, 200);

    wait_for_checks(&pool, id, 1, 5).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(checks(&pool, id).await.len(), 1);
}

#[tokio::test]
async fn paused_monitor_ignores_pings() {
    let (app, pool) = common::test_app_with_engine().await;
    let id = create(&app, &PUSH.replace("&active=on", "")).await;
    let path = format!("/push/{}", token(&pool, id).await.unwrap());
    assert_eq!(ping(&app, &path).await, 200);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(checks(&pool, id).await.is_empty());
}

#[tokio::test]
async fn missing_ping_goes_down() {
    let (app, pool) = common::test_app_with_engine().await;
    let form = PUSH
        .replace("interval_s=60", "interval_s=5")
        .replace("grace_s=60", "grace_s=0")
        .replace("retry_interval_s=20", "retry_interval_s=5");
    let id = create(&app, &form).await;
    wait_for_checks(&pool, id, 1, 8).await;
    let (ok, kind, _) = checks(&pool, id).await.remove(0);
    assert!(!ok);
    assert_eq!(kind.as_deref(), Some("missed"));
    assert_eq!(status(&pool, id).await, MonitorStatus::Down);
}

#[tokio::test]
async fn new_token_replaces_the_old_one() {
    let (app, pool) = common::test_app().await;
    let id = create(&app, PUSH).await;
    let old = token(&pool, id).await.unwrap();

    let r = post_form(&app, &format!("/monitors/{id}/push-token"), "").await;
    assert_eq!(r.status, 303);
    let new = token(&pool, id).await.unwrap();
    assert_ne!(new, old);
    assert_eq!(ping(&app, &format!("/push/{old}")).await, 404);
    assert_eq!(ping(&app, &format!("/push/{new}")).await, 200);
}

#[tokio::test]
async fn new_token_is_refused_for_an_http_monitor() {
    let (app, _) = common::test_app().await;
    let form = "name=api&kind=http&url=https%3A%2F%2Fexample.com&method=GET\
        &accepted_statuses=200-299&interval_s=60&retry_interval_s=20&timeout_s=10\
        &failure_threshold=1&active=on";
    let id = create(&app, form).await;
    let r = post_form(&app, &format!("/monitors/{id}/push-token"), "").await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn edit_keeps_the_token_and_switching_to_http_drops_it() {
    let (app, pool) = common::test_app().await;
    let id = create(&app, PUSH).await;
    let before = token(&pool, id).await.unwrap();

    let edit = format!("/monitors/{id}/edit");
    let r = post_form(
        &app,
        &edit,
        &PUSH.replace("interval_s=60", "interval_s=120"),
    )
    .await;
    assert_eq!(r.status, 303);
    assert_eq!(token(&pool, id).await, Some(before));

    let http = "name=backup&kind=http&url=https%3A%2F%2Fexample.com&method=GET\
        &accepted_statuses=200-299&interval_s=60&retry_interval_s=20&timeout_s=10\
        &failure_threshold=1&active=on";
    assert_eq!(post_form(&app, &edit, http).await.status, 303);
    assert_eq!(token(&pool, id).await, None);
}

#[tokio::test]
async fn failure_with_an_undecodable_message_is_still_a_failure() {
    let (app, pool) = common::test_app_with_engine().await;
    let id = create(&app, PUSH).await;
    let token = token(&pool, id).await.unwrap();
    let path = format!("/push/{token}?status=down&msg=%FF%FE");
    assert_eq!(ping(&app, &path).await, 200);
    wait_for_checks(&pool, id, 1, 5).await;
    assert_eq!(checks(&pool, id).await[0].1.as_deref(), Some("reported"));
}

#[tokio::test]
async fn overdue_monitor_misses_soon_after_a_restart() {
    let (app, pool) = common::test_app().await;
    let form = PUSH
        .replace("interval_s=60", "interval_s=3600")
        .replace("grace_s=60", "grace_s=1");
    let id = create(&app, &form).await;
    // Last ping two hours ago, recorded before the engine (re)starts.
    let two_hours_ago = uptime_monitoring::time::now_ms() - 7_200_000;
    sqlx::query("UPDATE monitor_state SET status = 'up', last_check_at = ? WHERE monitor_id = ?")
        .bind(two_hours_ago)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let _engine = uptime_monitoring::engine::start(
        pool.clone(),
        uptime_monitoring::engine::guard::Guard::new(false),
        uptime_monitoring::notify::Notifier::default(),
        Default::default(),
    )
    .await
    .unwrap();
    wait_for_checks(&pool, id, 1, 4).await;
    assert_eq!(checks(&pool, id).await[0].1.as_deref(), Some("missed"));
}

#[tokio::test]
async fn duplicate_of_a_push_monitor_gets_its_own_token() {
    let (app, pool) = common::test_app().await;
    let id = create(&app, PUSH).await;
    let original = token(&pool, id).await.unwrap();

    let r = get(&app, &format!("/monitors/{id}/duplicate")).await;
    assert!(r.body.contains(r#"value="push" selected"#));
    assert!(
        !r.body.contains(&original),
        "the copy form does not reveal the token"
    );

    let copy = create(&app, &PUSH.replace("name=backup", "name=backup%20(copy)")).await;
    assert_ne!(token(&pool, copy).await.unwrap(), original);
}
