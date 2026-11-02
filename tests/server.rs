//! Connection-level limits of the HTTP server: slow or idle clients cannot hold it.

use axum::Router;
use axum::routing::{get, post};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uptime_monitoring::server::{Limits, serve};

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n";

fn router() -> Router {
    Router::new()
        .route("/", get(|| async { "ok" }))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                "late"
            }),
        )
        .route("/body", post(|body: String| async move { body }))
}

async fn start(limits: Limits) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, router(), limits, std::future::pending()));
    addr
}

/// Everything the server sends until it closes the connection, or `None` past `within`.
async fn read_all(stream: &mut TcpStream, within: Duration) -> Option<String> {
    let mut out = Vec::new();
    tokio::time::timeout(within, stream.read_to_end(&mut out))
        .await
        .ok()?
        .ok()?;
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn limits() -> Limits {
    Limits {
        header_read: Duration::from_millis(300),
        request: Duration::from_millis(300),
        max_connections: 16,
        loopback_reserve: 0,
        shutdown_grace: Duration::from_secs(10),
    }
}

#[tokio::test]
async fn incomplete_headers_are_cut_off() {
    let addr = start(limits()).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: test\r\n")
        .await
        .unwrap();
    let answer = read_all(&mut stream, Duration::from_secs(3)).await;
    assert!(answer.is_some(), "the connection should be closed");
}

#[tokio::test]
async fn idle_connections_are_closed() {
    let addr = start(limits()).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    assert!(
        read_all(&mut stream, Duration::from_secs(3))
            .await
            .is_some()
    );
}

#[tokio::test]
async fn a_slow_body_gets_a_408() {
    let addr = start(limits()).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"POST /body HTTP/1.1\r\nHost: test\r\nContent-Length: 10\r\n\r\nab")
        .await
        .unwrap();
    let answer = read_all(&mut stream, Duration::from_secs(3)).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
}

#[tokio::test]
async fn a_slow_handler_gets_a_408() {
    let addr = start(limits()).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /slow HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let answer = read_all(&mut stream, Duration::from_secs(3)).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
}

#[tokio::test]
async fn connections_past_the_limit_are_closed_at_once() {
    let addr = start(Limits {
        header_read: Duration::from_secs(10),
        max_connections: 1,
        ..limits()
    })
    .await;
    let holder = TcpStream::connect(addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut refused = TcpStream::connect(addr).await.unwrap();
    let _ = refused.write_all(REQUEST).await;
    // Closed without an answer: an end of stream, or a reset since the request was unread.
    let mut out = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(1), refused.read_to_end(&mut out))
        .await
        .expect("closed at once, not left waiting");
    assert!(read.is_err() || out.is_empty(), "{read:?} {out:?}");

    drop(holder);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut next = TcpStream::connect(addr).await.unwrap();
    next.write_all(REQUEST).await.unwrap();
    let answer = read_all(&mut next, Duration::from_secs(3)).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}

/// The healthcheck connects through loopback: it still gets in when the limit is reached.
#[tokio::test]
async fn loopback_clients_have_a_reserve() {
    let addr = start(Limits {
        header_read: Duration::from_secs(10),
        max_connections: 1,
        loopback_reserve: 1,
        ..limits()
    })
    .await;
    let _holder = TcpStream::connect(addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut health = TcpStream::connect(addr).await.unwrap();
    health.write_all(REQUEST).await.unwrap();
    let answer = read_all(&mut health, Duration::from_secs(3)).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}

/// A request that never ends does not keep the process from exiting.
#[tokio::test]
async fn shutdown_waits_at_most_the_grace_period() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let limits = Limits {
        request: Duration::from_secs(60),
        shutdown_grace: Duration::from_millis(200),
        ..limits()
    };
    let server = tokio::spawn(serve(listener, router(), limits, async {
        let _ = stopped.await;
    }));

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /slow HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("the server should exit within its grace period")
        .unwrap();
}

#[tokio::test]
async fn a_normal_request_is_served() {
    let addr = start(limits()).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(REQUEST).await.unwrap();
    let answer = read_all(&mut stream, Duration::from_secs(3)).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("ok"), "{answer}");
}
