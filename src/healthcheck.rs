//! `uptime-monitoring healthcheck`: asks the running instance for `/health`. The Docker image
//! has no shell or curl, so its `HEALTHCHECK` runs the binary itself.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(5);

/// The address to probe for a server bound to `bind`: loopback when it listens everywhere.
pub fn target(bind: SocketAddr) -> SocketAddr {
    let mut addr = bind;
    if bind.ip().is_unspecified() {
        addr.set_ip(match bind {
            SocketAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
            SocketAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
        });
    }
    addr
}

/// `Ok` when `GET /health` answers 200, otherwise what went wrong.
pub async fn probe(addr: SocketAddr) -> Result<(), String> {
    tokio::time::timeout(TIMEOUT, request(addr))
        .await
        .map_err(|_| format!("no answer from {addr} within {TIMEOUT:?}"))?
}

async fn request(addr: SocketAddr) -> Result<(), String> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("cannot connect to {addr}: {e}"))?;
    stream
        .write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .map_err(|e| e.to_string())?;
    // The status line may arrive in several reads; it fits well within 64 bytes.
    let mut head = Vec::with_capacity(64);
    let mut buf = [0u8; 64];
    while head.len() < 64 && !head.contains(&b'\n') {
        let read = stream.read(&mut buf).await.map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&buf[..read]);
    }
    let status_line = String::from_utf8_lossy(&head);
    let status_line = status_line.lines().next().unwrap_or_default();
    match status_line.split(' ').nth(1) {
        Some("200") => Ok(()),
        _ => Err(format!("unhealthy: {status_line}")),
    }
}
