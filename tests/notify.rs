mod common;

use axum::Router;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::routing::{get, post};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use uptime_monitoring::engine::guard::Guard;
use uptime_monitoring::forms::monitor::MonitorForm;
use uptime_monitoring::models::{Monitor, NotificationLog};
use uptime_monitoring::notify::channel::{ChannelConfig, WebhookConfig};
use uptime_monitoring::notify::{Event, MonitorRef, Notification, Notifier};

struct Received {
    path: String,
    headers: HeaderMap,
    body: serde_json::Value,
}

/// A server that answers `statuses` in turn (then 200) and reports each request.
async fn receiver(statuses: Vec<u16>) -> (String, mpsc::UnboundedReceiver<Received>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/hook/{*rest}",
        post(move |uri: Uri, headers: HeaderMap, body: String| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(Received {
                path: uri.path().to_string(),
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
    (format!("http://{addr}/hook/x"), rx)
}

async fn monitor(pool: &SqlitePool) -> MonitorRef {
    monitor_of(pool, "https://api.example.com").await
}

async fn monitor_of(pool: &SqlitePool, url: &str) -> MonitorRef {
    named(pool, "API", url).await
}

async fn named(pool: &SqlitePool, name: &str, url: &str) -> MonitorRef {
    let input = MonitorForm {
        name: name.to_string(),
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
        url: Some(input.url),
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
    notifier_with_public_url(pool, None)
}

fn notifier_with_public_url(pool: &SqlitePool, public_url: Option<&str>) -> Notifier {
    Notifier::start_with(
        pool.clone(),
        Guard::new(false),
        public_url.map(str::to_string),
        vec![
            Duration::ZERO,
            Duration::from_millis(10),
            Duration::from_millis(10),
        ],
        GROUP_WINDOW,
    )
}

/// Short, so that tests stay fast.
const GROUP_WINDOW: Duration = Duration::from_millis(100);

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
    assert!(request.body.get("click").is_none());

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
    assert!(request.body["embeds"][0].get("url").is_none());
}

#[tokio::test]
async fn telegram_sends_a_text_message_to_its_chat() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![]).await;
    let m = monitor(&pool).await;
    let telegram = serde_json::json!({ "server": url, "token": "123:ABC", "chat_id": "-1001" });
    link(
        &pool,
        m.id,
        channel(&pool, "telegram", telegram, true).await,
    )
    .await;

    notifier_with_public_url(&pool, Some("https://up.example.com")).notify(down(&m));
    let request = next(&mut rx).await;
    assert_eq!(request.path, "/hook/x/bot123:ABC/sendMessage");
    assert_eq!(request.body["chat_id"], "-1001");
    let text = request.body["text"].as_str().unwrap();
    assert!(text.contains("API is down"), "{text}");
    assert!(text.contains("HTTP 503"), "{text}");
    assert!(
        text.ends_with(&format!("https://up.example.com/monitors/{}", m.id)),
        "{text}"
    );
    assert_eq!(wait_for_log(&pool, 1).await[0].error, None);
}

#[tokio::test]
async fn telegram_errors_carry_its_description() {
    let pool = common::test_app().await.1;
    let app = Router::new().route(
        "/{*rest}",
        post(|| async {
            (
                StatusCode::BAD_REQUEST,
                r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#,
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let m = monitor(&pool).await;
    let telegram = serde_json::json!({
        "server": format!("http://{addr}"), "token": "123:SECRET", "chat_id": "-1001"
    });
    link(
        &pool,
        m.id,
        channel(&pool, "telegram", telegram, true).await,
    )
    .await;

    notifier(&pool).notify(down(&m));
    let log = wait_for_log(&pool, 1).await;
    assert_eq!(
        log[0].error.as_deref(),
        Some("HTTP 400: Bad Request: chat not found")
    );
}

#[tokio::test]
async fn public_url_links_every_channel_to_the_detail_page() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![]).await;
    let m = monitor(&pool).await;
    for (kind, config) in [
        ("webhook", serde_json::json!({ "url": url })),
        ("discord", serde_json::json!({ "url": url })),
        (
            "ntfy",
            serde_json::json!({ "server": url, "topic": "alerts" }),
        ),
    ] {
        link(&pool, m.id, channel(&pool, kind, config, true).await).await;
    }

    notifier_with_public_url(&pool, Some("https://status.example.com")).notify(down(&m));
    let expected = format!("https://status.example.com/monitors/{}", m.id);
    let mut links = Vec::new();
    for _ in 0..3 {
        let body = next(&mut rx).await.body;
        if body.get("embeds").is_some() {
            links.push(("discord", body["embeds"][0]["url"].clone()));
        } else if body.get("topic").is_some() {
            links.push(("ntfy", body["click"].clone()));
        } else {
            links.push(("webhook", body["monitor"]["link"].clone()));
        }
    }
    links.sort_by_key(|(kind, _)| *kind);
    let kinds: Vec<_> = links.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(kinds, ["discord", "ntfy", "webhook"]);
    for (kind, link) in links {
        assert_eq!(link, expected.as_str(), "{kind}");
    }
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
async fn tests_send_once_without_logging_and_respect_the_guard() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![]).await;
    let config = ChannelConfig::Webhook(WebhookConfig { url });

    let blocking = Notifier::start(pool.clone(), Guard::new(true), None);
    let error = blocking.send_test(&config).await.unwrap_err();
    assert!(error.contains("127.0.0.1"), "{error}");
    assert!(rx.try_recv().is_err());

    Notifier::default().send_test(&config).await.unwrap();
    assert_eq!(next(&mut rx).await.body["event"], "test");
    assert!(NotificationLog::recent(&pool, 50).await.unwrap().is_empty());
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
    let engine = uptime_monitoring::engine::start(
        pool.clone(),
        Guard::new(false),
        notifier,
        Default::default(),
    )
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
        None,
        vec![Duration::ZERO, Duration::from_millis(300)],
        GROUP_WINDOW,
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

#[tokio::test]
async fn simultaneous_downs_share_one_message_except_on_webhooks() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![]).await;
    let api = monitor(&pool).await;
    let web = named(&pool, "Web", "https://web.example.com").await;
    let discord = channel(&pool, "discord", serde_json::json!({ "url": url }), true).await;
    let webhook = channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await;
    for m in [&api, &web] {
        link(&pool, m.id, discord).await;
        link(&pool, m.id, webhook).await;
    }

    let notifier = notifier(&pool);
    notifier.notify(down(&api));
    notifier.notify(down(&web));
    let mut embeds = Vec::new();
    let mut webhook_monitors = Vec::new();
    for _ in 0..3 {
        let body = next(&mut rx).await.body;
        match body.get("embeds") {
            Some(embeds_of) => embeds.push(embeds_of[0].clone()),
            None => webhook_monitors.push(body["monitor"]["name"].as_str().unwrap().to_string()),
        }
    }
    assert_eq!(embeds.len(), 1, "one Discord message for both");
    assert_eq!(embeds[0]["title"], "2 monitors are down");
    assert_eq!(embeds[0]["description"], "API: HTTP 503\nWeb: HTTP 503");
    webhook_monitors.sort();
    assert_eq!(
        webhook_monitors,
        ["API", "Web"],
        "webhooks get one request each"
    );

    let log = wait_for_log(&pool, 4).await;
    assert!(log.iter().all(|l| l.ok && l.event == "down"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rx.try_recv().is_err(), "nothing more");
}

#[tokio::test]
async fn a_retried_alert_holds_up_only_its_own_monitor() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![500]).await;
    let api = monitor(&pool).await;
    let web = named(&pool, "Web", "https://web.example.com").await;
    let c = channel(&pool, "webhook", serde_json::json!({ "url": url }), true).await;
    link(&pool, api.id, c).await;
    link(&pool, web.id, c).await;
    let notifier = Notifier::start_with(
        pool.clone(),
        Guard::new(false),
        None,
        vec![Duration::ZERO, Duration::from_millis(500)],
        Duration::ZERO,
    );
    notifier.notify(down(&api));
    let first = next(&mut rx).await.body;
    assert_eq!(first["monitor"]["name"], "API", "answered 500");
    notifier.notify(down(&web));
    let second = next(&mut rx).await.body;
    assert_eq!(second["monitor"]["name"], "Web", "sent before API's retry");
    assert_eq!(next(&mut rx).await.body["monitor"]["name"], "API");
}

#[tokio::test]
async fn channels_without_grouping_send_at_once() {
    let pool = common::test_app().await.1;
    let (url, mut rx) = receiver(vec![]).await;
    let api = monitor(&pool).await;
    let web = named(&pool, "Web", "https://web.example.com").await;
    let c = channel(&pool, "discord", serde_json::json!({ "url": url }), true).await;
    sqlx::query("UPDATE notification_channels SET group_alerts = 0")
        .execute(&pool)
        .await
        .unwrap();
    link(&pool, api.id, c).await;
    link(&pool, web.id, c).await;
    // A window longer than `next` waits: only a channel that does not group answers in time.
    let notifier = Notifier::start_with(
        pool.clone(),
        Guard::new(false),
        None,
        vec![Duration::ZERO],
        Duration::from_secs(60),
    );
    notifier.notify(down(&api));
    notifier.notify(down(&web));
    let mut titles = vec![
        next(&mut rx).await.body["embeds"][0]["title"].clone(),
        next(&mut rx).await.body["embeds"][0]["title"].clone(),
    ];
    titles.sort_by_key(|t| t.to_string());
    assert_eq!(titles, ["API is down", "Web is down"]);
}

/// One mail as an SMTP server received it.
struct Mail {
    /// The name given with EHLO.
    ehlo: String,
    from: String,
    to: Vec<String>,
    data: String,
}

/// A plain SMTP server that answers `MAIL FROM` with `codes` in turn (then 250), one per
/// connection, and reports each accepted mail.
async fn smtp_server(codes: Vec<u16>) -> (u16, mpsc::UnboundedReceiver<Mail>, Arc<AtomicUsize>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (tx, rx) = mpsc::unbounded_channel();
    let connections = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = connections.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let n = count.fetch_add(1, Ordering::SeqCst);
            let code = codes.get(n).copied().unwrap_or(250);
            let tx = tx.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                write.write_all(b"220 test ESMTP\r\n").await.unwrap();
                let mut mail = Mail {
                    ehlo: String::new(),
                    from: String::new(),
                    to: vec![],
                    data: String::new(),
                };
                while let Ok(Some(line)) = lines.next_line().await {
                    let upper = line.to_ascii_uppercase();
                    let reply = if upper.starts_with("EHLO") {
                        mail.ehlo = line[5..].to_string();
                        "250-test\r\n250 8BITMIME\r\n".to_string()
                    } else if upper.starts_with("MAIL FROM:") {
                        mail.from = line[10..].to_string();
                        format!("{code} {}\r\n", if code == 250 { "OK" } else { "refused" })
                    } else if upper.starts_with("RCPT TO:") {
                        mail.to.push(line[8..].to_string());
                        "250 OK\r\n".to_string()
                    } else if upper == "DATA" {
                        write.write_all(b"354 go on\r\n").await.unwrap();
                        while let Ok(Some(data)) = lines.next_line().await {
                            if data == "." {
                                break;
                            }
                            mail.data.push_str(&data);
                            mail.data.push('\n');
                        }
                        let ehlo = mail.ehlo.clone();
                        let _ = tx.send(std::mem::replace(
                            &mut mail,
                            Mail {
                                ehlo,
                                from: String::new(),
                                to: vec![],
                                data: String::new(),
                            },
                        ));
                        "250 queued\r\n".to_string()
                    } else if upper == "QUIT" {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        break;
                    } else {
                        "250 OK\r\n".to_string()
                    };
                    if write.write_all(reply.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (port, rx, connections)
}

fn email_config(port: u16) -> serde_json::Value {
    serde_json::json!({
        "host": "127.0.0.1",
        "port": port,
        "security": "none",
        "from": "Uptime <up@example.com>",
        "to": ["ops@example.com", "dev@example.com"],
    })
}

async fn next_mail(rx: &mut mpsc::UnboundedReceiver<Mail>) -> Mail {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("a mail within 5 s")
        .unwrap()
}

#[tokio::test]
async fn email_sends_a_plain_text_mail_to_every_recipient() {
    let pool = common::test_app().await.1;
    let (port, mut rx, _) = smtp_server(vec![]).await;
    let m = monitor(&pool).await;
    link(
        &pool,
        m.id,
        channel(&pool, "email", email_config(port), true).await,
    )
    .await;

    notifier_with_public_url(&pool, Some("https://up.example.com")).notify(down(&m));
    let mail = next_mail(&mut rx).await;
    assert_eq!(mail.ehlo, "up.example.com", "the public name");
    assert_eq!(mail.from, "<up@example.com>");
    assert_eq!(mail.to, ["<ops@example.com>", "<dev@example.com>"]);
    assert!(mail.data.contains("Subject: API is down"), "{}", mail.data);
    assert!(
        mail.data.contains("From: Uptime <up@example.com>"),
        "{}",
        mail.data
    );
    assert!(
        mail.data.contains("text/plain; charset=utf-8"),
        "{}",
        mail.data
    );
    assert!(mail.data.contains("HTTP 503"), "{}", mail.data);
    assert!(
        mail.data
            .contains(&format!("https://up.example.com/monitors/{}", m.id)),
        "{}",
        mail.data
    );
    assert_eq!(wait_for_log(&pool, 1).await[0].error, None);
}

#[tokio::test]
async fn email_retries_transient_refusals_but_not_permanent_ones() {
    let pool = common::test_app().await.1;
    let (port, mut rx, _) = smtp_server(vec![451]).await;
    let m = monitor(&pool).await;
    link(
        &pool,
        m.id,
        channel(&pool, "email", email_config(port), true).await,
    )
    .await;
    notifier(&pool).notify(down(&m));
    next_mail(&mut rx).await;
    let log = wait_for_log(&pool, 1).await;
    assert!(
        log[0].ok,
        "the second attempt succeeded: {:?}",
        log[0].error
    );

    sqlx::query("DELETE FROM monitor_channels")
        .execute(&pool)
        .await
        .unwrap();
    let (port, _rx, connections) = smtp_server(vec![550]).await;
    link(
        &pool,
        m.id,
        channel(&pool, "email", email_config(port), true).await,
    )
    .await;
    notifier(&pool).notify(down(&m));
    let log = wait_for_log(&pool, 2).await;
    assert!(!log[0].ok);
    let error = log[0].error.as_deref().unwrap();
    assert!(error.contains("550"), "{error}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "no retry after a 550"
    );
}

#[tokio::test]
async fn email_respects_the_guard() {
    let (port, mut rx, connections) = smtp_server(vec![]).await;
    let pool = common::test_app().await.1;
    let config: ChannelConfig =
        ChannelConfig::parse("email", &email_config(port).to_string()).unwrap();

    let blocking = Notifier::start(pool, Guard::new(true), None);
    let error = blocking.send_test(&config).await.unwrap_err();
    assert!(error.contains("127.0.0.1"), "{error}");
    assert_eq!(connections.load(Ordering::SeqCst), 0);

    Notifier::default().send_test(&config).await.unwrap();
    let mail = next_mail(&mut rx).await;
    assert!(mail.data.contains("Subject: "));
    assert_eq!(
        mail.ehlo, "[127.0.0.1]",
        "an address literal without PUBLIC_URL"
    );
}

#[tokio::test]
async fn email_tries_every_address_of_the_relay() {
    // `localhost` usually resolves to ::1 first; the server only listens on 127.0.0.1.
    let (port, mut rx, _) = smtp_server(vec![]).await;
    let mut config = email_config(port);
    config["host"] = serde_json::json!("localhost");
    let config = ChannelConfig::parse("email", &config.to_string()).unwrap();
    Notifier::default().send_test(&config).await.unwrap();
    next_mail(&mut rx).await;
}
