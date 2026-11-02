//! Connection-level limits of the HTTP server: slow or idle clients cannot hold it.

use axum::Router;
use axum::routing::{get, post};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uptime_monitoring::server::{Limits, serve};

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n";

async fn start(limits: Limits) -> std::net::SocketAddr {
    let router = Router::new()
        .route("/", get(|| async { "ok" }))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                "late"
            }),
        )
        .route("/body", post(|body: String| async move { body }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, router, limits, std::future::pending()));
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
async fn connections_past_the_limit_wait_for_a_free_slot() {
    let addr = start(Limits {
        header_read: Duration::from_secs(10),
        max_connections: 1,
        ..limits()
    })
    .await;
    let holder = TcpStream::connect(addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut waiting = TcpStream::connect(addr).await.unwrap();
    waiting.write_all(REQUEST).await.unwrap();
    assert_eq!(
        read_all(&mut waiting, Duration::from_millis(500)).await,
        None,
        "served while the only slot was taken"
    );

    drop(holder);
    let answer = read_all(&mut waiting, Duration::from_secs(3))
        .await
        .unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
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
