//! The HTTP/1.1 server, with the limits `axum::serve` does not set: a client that sends its
//! headers or body slowly, or keeps connections open doing nothing, cannot hold the server.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tower::ServiceExt;
use tower_http::timeout::TimeoutLayer;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// To receive a request's headers, counted from the moment the server waits for them: an
    /// idle keep-alive connection is closed after this long too.
    pub header_read: Duration,
    /// To read the body and produce the response (408 past it). A streamed response body,
    /// such as the backup, is not counted.
    pub request: Duration,
    /// Connections served at once; the next ones wait in the listen backlog.
    pub max_connections: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            header_read: Duration::from_secs(10),
            request: Duration::from_secs(30),
            max_connections: 512,
        }
    }
}

/// Serves `router` until `shutdown` completes, then waits for the requests in flight.
/// Handlers see the peer address as `ConnectInfo<SocketAddr>`.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    limits: Limits,
    shutdown: impl Future<Output = ()>,
) {
    let router = router.layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        limits.request,
    ));
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    let graceful = GracefulShutdown::new();
    let mut shutdown = pin!(shutdown);

    loop {
        let slot = tokio::select! {
            slot = slots.clone().acquire_owned() => slot.expect("never closed"),
            () = &mut shutdown => break,
        };
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(e) => {
                    // Out of file descriptors, for instance: wait instead of spinning.
                    tracing::warn!(error = %e, "cannot accept a connection");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let _ = stream.set_nodelay(true);

        let service = router
            .clone()
            .map_request(move |mut request: Request<Incoming>| {
                request.extensions_mut().insert(ConnectInfo(peer));
                request.map(Body::new)
            });
        let connection = hyper::server::conn::http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(limits.header_read)
            .serve_connection(TokioIo::new(stream), TowerToHyperService::new(service));
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::debug!(error = %e, %peer, "connection ended");
            }
            drop(slot);
        });
    }

    drop(listener);
    graceful.shutdown().await;
}
