mod common;

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use uptime_monitoring::engine::guard::Guard;
use uptime_monitoring::forms::monitor::MonitorForm;
use uptime_monitoring::models::{Channel, Monitor, NotificationLog};
use uptime_monitoring::notify::{Event, MonitorRef, Notification, Notifier};

struct Received {
    headers: HeaderMap,
    body: serde_json::Value,
}

/// A server that answers `statuses` in turn (then 200) and reports each request.
async fn receiver(statuses: Vec<u16>) -> (String, mpsc::UnboundedReceiver<Received>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/hook",
        post(move |headers: HeaderMap, body: String| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(Received {
                headers,
                body: serde_json::from_str(&body).unwrap(),
            });
            let status = statuses.get(n).copied().unwrap_or(200);
            async move { StatusCode::from_u16(status).unwrap() }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/hook"), rx)
}

async fn monitor(pool: &SqlitePool) -> MonitorRef {
    monitor_of(pool, "https://api.example.com").await
}

async fn monitor_of(pool: &SqlitePool, url: &str) -> MonitorRef {
    let input = MonitorForm {
        name: "API".to_string(),
        url: url.to_string(),
        failure_threshold: "1".to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap();
    let id = Monitor::create(pool, &input).await.unwrap();
    MonitorRef {
        id,
        name: input.name,
        url: input.url,
    }
}

/// Inserts a channel directly: forms refuse local Discord URLs.
async fn channel(pool: &SqlitePool, kind: &str, config: serde_json::Value, enabled: bool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO notification_channels (name, kind, config, enabled, created_at)
         VALUES (?, ?, ?, ?, 0) RETURNING id",
    )
    .bind(format!("{kind} {config}"))
    .bind(kind)
    .bind(config.to_string())
    .bind(enabled)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn link(pool: &SqlitePool, monitor_id: i64, channel_id: i64) {
    sqlx::query("INSERT INTO monitor_channels (monitor_id, channel_id) VALUES (?, ?)")
        .bind(monitor_id)
        .bind(channel_id)
        .execute(pool)
        .await
        .unwrap();
}

fn notifier(pool: &SqlitePool) -> Notifier {
    Notifier::start_with(
        pool.clone(),
        Guard::new(false),
        vec![
            Duration::ZERO,
            Duration::from_millis(10),
            Duration::from_millis(10),
        ],
    )
}

fn down(monitor: &MonitorRef) -> Notification {
    Notification {
        monitor: Some(monitor.clone()),
        event: Event::Down {
            since: 1_000,
            cause: "HTTP 503".to_string(),
        },
        at: 2_000,
    }
}

async fn next(rx: &mut mpsc::UnboundedReceiver<Received>) -> Received {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("a request within 5 s")
        .unwrap()
}

/// Waits up to 5 s for `n` log rows.
async fn wait_for_log(pool: &SqlitePool, n: usize) -> Vec<uptime_monitoring::models::LogEntry> {
    for _ in 0..100 {
        let log = NotificationLog::recent(pool, 50).await.unwrap();
        if log.len() >= n {
            return log;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the log never reached {n} rows");
}

#[tokio::test]
async fn webhook_receives_the_event_as_json() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![]).await;
    let m = monitor(&pool).await;
    let c = channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await;
    link(&pool, m.id, c).await;

    notifier(&pool).notify(down(&m));
    let request = next(&mut rx).await;
    assert_eq!(request.body["event"], "down");
    assert_eq!(request.body["monitor"]["name"], "API");
    assert_eq!(request.body["title"], "API is down");
    assert_eq!(request.body["at"], 2_000);

    let log = wait_for_log(&pool, 1).await;
    assert!(log[0].ok);
    assert_eq!(log[0].event, "down");
    assert_eq!(log[0].monitor_name.as_deref(), Some("API"));
}

#[tokio::test]
async fn ntfy_and_discord_get_their_own_format() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![]).await;
    let m = monitor(&pool).await;
    let ntfy = serde_json::json!({ "server": url, "topic": "alerts", "token": "tk_1" });
    link(&pool, m.id, channel(&pool, "ntfy", ntfy, true).await).await;

    notifier(&pool).notify(down(&m));
    let request = next(&mut rx).await;
    assert_eq!(request.headers["authorization"], "Bearer tk_1");
    assert_eq!(request.body["topic"], "alerts");
    assert_eq!(request.body["title"], "API is down");
    assert_eq!(request.body["priority"], 4);

    sqlx::query("DELETE FROM monitor_channels")
        .execute(&pool)
        .await
        .unwrap();
    let discord = serde_json::json!({ "url": url });
    link(&pool, m.id, channel(&pool, "discord", discord, true).await).await;
    notifier(&pool).notify(down(&m));
    let request = next(&mut rx).await;
    assert_eq!(request.body["embeds"][0]["title"], "API is down");
    assert_eq!(request.body["embeds"][0]["color"], 0xE5484D);
}

#[tokio::test]
async fn server_errors_are_retried_and_client_errors_are_not() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![500, 503]).await;
    let m = monitor(&pool).await;
    link(
        &pool,
        m.id,
        channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await,
    )
    .await;
    notifier(&pool).notify(down(&m));
    for _ in 0..3 {
        next(&mut rx).await;
    }
    let log = wait_for_log(&pool, 1).await;
    assert!(log[0].ok, "the third attempt succeeded: {:?}", log[0].error);

    let (url, mut rx) = receiver(vec![404]).await;
    sqlx::query("DELETE FROM monitor_channels")
        .execute(&pool)
        .await
        .unwrap();
    link(
        &pool,
        m.id,
        channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await,
    )
    .await;
    notifier(&pool).notify(down(&m));
    next(&mut rx).await;
    let log = wait_for_log(&pool, 2).await;
    assert!(!log[0].ok);
    assert_eq!(log[0].error.as_deref(), Some("HTTP 404"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rx.try_recv().is_err(), "no retry after a 404");
}

#[tokio::test]
async fn only_enabled_linked_channels_are_used() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![]).await;
    let m = monitor(&pool).await;
    let disabled = channel(
        &pool,
        "webhook",
        serde_json::json!({ "url": format!("{url}?d") }),
        false,
    )
    .await;
    link(&pool, m.id, disabled).await;
    channel(
        &pool,
        "webhook",
        serde_json::json!({ "url": format!("{url}?unlinked") }),
        true,
    )
    .await;

    notifier(&pool).notify(down(&m));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rx.try_recv().is_err());
    assert!(NotificationLog::recent(&pool, 50).await.unwrap().is_empty());
}

#[tokio::test]
async fn private_targets_are_refused_when_blocked() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![]).await;
    let id = channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await;
    let channel = Channel::find(&pool, id).await.unwrap().unwrap();

    let blocking = Notifier::start(pool.clone(), Guard::new(true));
    blocking.send_test(&pool, &channel).await.unwrap();
    let log = NotificationLog::recent(&pool, 50).await.unwrap();
    assert!(!log[0].ok);
    assert!(
        log[0].error.as_deref().unwrap().contains("127.0.0.1"),
        "{:?}",
        log[0].error
    );
    assert!(rx.try_recv().is_err());

    Notifier::default()
        .send_test(&pool, &channel)
        .await
        .unwrap();
    assert_eq!(next(&mut rx).await.body["event"], "test");
    let log = NotificationLog::recent(&pool, 50).await.unwrap();
    assert!(log[0].ok);
    assert_eq!(
        (log[0].event.as_str(), log[0].monitor_name.as_deref()),
        ("test", None)
    );
}

#[tokio::test]
async fn engine_alerts_on_down_then_up() {
    let pool = common::test_app().await.1;
    let failing = Arc::new(AtomicBool::new(true));
    let flag = failing.clone();
    let app = Router::new().route(
        "/health",
        get(move || {
            let status = if flag.load(Ordering::SeqCst) {
                503
            } else {
                200
            };
            async move { StatusCode::from_u16(status).unwrap() }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}/health", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let (url, mut rx) = receiver(vec![]).await;
    let m = monitor_of(&pool, &target).await;
    link(
        &pool,
        m.id,
        channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await,
    )
    .await;
    let notifier = notifier(&pool);
    let engine = uptime_monitoring::engine::start(pool.clone(), Guard::new(false), notifier)
        .await
        .unwrap();

    engine.reload(m.id);
    let request = next(&mut rx).await;
    assert_eq!(request.body["event"], "down");
    assert!(
        request.body["message"]
            .as_str()
            .unwrap()
            .starts_with("HTTP 503"),
        "{}",
        request.body["message"]
    );

    failing.store(false, Ordering::SeqCst);
    engine.reload(m.id);
    assert_eq!(next(&mut rx).await.body["event"], "up");
}

fn up(monitor: &MonitorRef) -> Notification {
    Notification {
        monitor: Some(monitor.clone()),
        event: Event::Up { down_since: 1_000 },
        at: 3_000,
    }
}

#[tokio::test]
async fn alerts_of_a_monitor_arrive_in_order_despite_retries() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![500]).await;
    let m = monitor(&pool).await;
    link(
        &pool,
        m.id,
        channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await,
    )
    .await;
    let notifier = notifier(&pool);
    notifier.notify(down(&m));
    notifier.notify(up(&m));
    let events: Vec<_> = [
        next(&mut rx).await,
        next(&mut rx).await,
        next(&mut rx).await,
    ]
    .iter()
    .map(|r| r.body["event"].as_str().unwrap().to_string())
    .collect();
    assert_eq!(
        events,
        ["down", "down", "up"],
        "the retried down comes before up"
    );
}

#[tokio::test]
async fn delivery_errors_do_not_reveal_the_url() {
    let pool = common::test_app().await.1;
    // Nothing listens on port 9 of localhost.
    let url = "http://127.0.0.1:9/api/webhooks/1/SECRET";
    let m = monitor(&pool).await;
    link(
        &pool,
        m.id,
        channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await,
    )
    .await;
    notifier(&pool).notify(down(&m));
    let log = wait_for_log(&pool, 1).await;
    let error = log[0].error.as_deref().unwrap();
    assert!(!error.contains("SECRET"), "{error}");
}

#[tokio::test]
async fn channel_deleted_during_retries_still_logs() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![500, 500, 500]).await;
    let m = monitor(&pool).await;
    let c = channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await;
    link(&pool, m.id, c).await;
    let notifier = Notifier::start_with(
        pool.clone(),
        Guard::new(false),
        vec![Duration::ZERO, Duration::from_millis(300)],
    );
    notifier.notify(down(&m));
    next(&mut rx).await;
    sqlx::query("DELETE FROM notification_channels")
        .execute(&pool)
        .await
        .unwrap();
    let log = wait_for_log(&pool, 1).await;
    assert_eq!((log[0].ok, log[0].channel_name.as_deref()), (false, None));
}
