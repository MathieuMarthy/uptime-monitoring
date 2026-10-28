use axum::Router;
use axum::response::Redirect;
use axum::routing::get;
use std::time::Duration;
use uptime_monitoring::engine::guard::Guard;
use uptime_monitoring::engine::http::HttpChecker;
use uptime_monitoring::engine::verdict::{CheckOutcome, ErrorKind};
use uptime_monitoring::models::Monitor;

/// Starts a local target server and returns its base URL.
async fn target() -> String {
    let app = Router::new()
        .route("/ok", get(|| async { "hello world" }))
        .route(
            "/error",
            get(|| async { (http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                "late"
            }),
        )
        .route("/redirect", get(|| async { Redirect::to("/ok") }))
        .route("/loop", get(|| async { Redirect::to("/loop") }))
        .route(
            "/big",
            get(|| async { format!("{}needle", "a".repeat(2 * 1024 * 1024)) }),
        )
        .route(
            "/echo",
            axum::routing::post(|headers: http::HeaderMap, body: String| async move {
                let token = headers
                    .get("x-token")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                format!("{token}:{body}")
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn monitor(url: &str) -> Monitor {
    Monitor {
        id: 1,
        name: "target".to_string(),
        kind: "http".to_string(),
        url: url.to_string(),
        method: "GET".to_string(),
        headers: None,
        body: None,
        keyword: None,
        accepted_statuses: "200-299".to_string(),
        follow_redirects: true,
        ignore_tls_errors: false,
        interval_s: 60,
        retry_interval_s: 20,
        timeout_s: 2,
        failure_threshold: 3,
        tls_expiry_warn_days: Some(14),
        paused: false,
        created_at: 0,
        updated_at: 0,
    }
}

async fn check(m: &Monitor) -> CheckOutcome {
    HttpChecker::new(Guard::new(false)).check(m).await
}

fn kind(outcome: &CheckOutcome) -> Option<ErrorKind> {
    outcome.failure.as_ref().map(|f| f.kind)
}

#[tokio::test]
async fn successful_check() {
    let base = target().await;
    let o = check(&monitor(&format!("{base}/ok"))).await;
    assert!(o.ok(), "{o:?}");
    assert_eq!(o.status_code, Some(200));
    assert!(o.latency_ms.is_some());
    assert_eq!(o.tls_expires_at, None);
}

#[tokio::test]
async fn unexpected_status_fails() {
    let base = target().await;
    let o = check(&monitor(&format!("{base}/error"))).await;
    assert_eq!(kind(&o), Some(ErrorKind::Status));
    assert_eq!(o.status_code, Some(500));
    let accepted = Monitor {
        accepted_statuses: "200-299,500".to_string(),
        ..monitor(&format!("{base}/error"))
    };
    assert!(check(&accepted).await.ok());
}

#[tokio::test]
async fn keyword_presence() {
    let base = target().await;
    let with = |keyword: &str, path: &str| Monitor {
        keyword: Some(keyword.to_string()),
        ..monitor(&format!("{base}{path}"))
    };
    assert!(check(&with("world", "/ok")).await.ok());
    let missing = check(&with("absent", "/ok")).await;
    assert_eq!(kind(&missing), Some(ErrorKind::Keyword));
    let beyond_limit = check(&with("needle", "/big")).await;
    assert_eq!(kind(&beyond_limit), Some(ErrorKind::Keyword));
}

#[tokio::test]
async fn timeout() {
    let base = target().await;
    let m = Monitor {
        timeout_s: 1,
        ..monitor(&format!("{base}/slow"))
    };
    let o = check(&m).await;
    assert_eq!(kind(&o), Some(ErrorKind::Timeout));
}

#[tokio::test]
async fn connection_refused() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let o = check(&monitor(&format!("http://{addr}/"))).await;
    assert_eq!(kind(&o), Some(ErrorKind::Connect), "{o:?}");
    assert_eq!(o.latency_ms, None);
}

#[tokio::test]
async fn unknown_host() {
    let o = check(&monitor("http://does-not-exist.invalid/")).await;
    assert_eq!(kind(&o), Some(ErrorKind::Dns), "{o:?}");
}

#[tokio::test]
async fn redirects() {
    let base = target().await;
    assert_eq!(
        check(&monitor(&format!("{base}/redirect")))
            .await
            .status_code,
        Some(200)
    );
    let not_followed = Monitor {
        follow_redirects: false,
        ..monitor(&format!("{base}/redirect"))
    };
    let o = check(&not_followed).await;
    assert_eq!(
        (o.status_code, kind(&o)),
        (Some(303), Some(ErrorKind::Status))
    );
    let o = check(&monitor(&format!("{base}/loop"))).await;
    assert_eq!(kind(&o), Some(ErrorKind::Connect), "{o:?}");
}

#[tokio::test]
async fn method_headers_and_body_are_sent() {
    let base = target().await;
    let m = Monitor {
        method: "POST".to_string(),
        headers: Some(r#"{"X-Token": "secret"}"#.to_string()),
        body: Some("payload".to_string()),
        keyword: Some("secret:payload".to_string()),
        ..monitor(&format!("{base}/echo"))
    };
    let o = check(&m).await;
    assert!(o.ok(), "{o:?}");
}

#[tokio::test]
async fn private_targets_blocked_when_enabled() {
    let base = target().await;
    let blocking = HttpChecker::new(Guard::new(true));
    let o = blocking.check(&monitor(&format!("{base}/ok"))).await;
    assert_eq!(kind(&o), Some(ErrorKind::Blocked), "{o:?}");
    let by_name = base.replace("127.0.0.1", "localhost");
    let o = blocking.check(&monitor(&format!("{by_name}/ok"))).await;
    assert_eq!(kind(&o), Some(ErrorKind::Blocked), "{o:?}");
}
