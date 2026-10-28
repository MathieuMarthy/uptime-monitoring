//! Runs one HTTP(S) check and turns what happened into a `CheckOutcome`.

use crate::engine::guard::{Guard, GuardError};
use crate::engine::verdict::{CheckOutcome, ErrorKind, Failure};
use crate::forms::monitor::parse_status_ranges;
use crate::models::Monitor;
use crate::time::now_ms;
use reqwest::{Client, Method, Url};
use std::error::Error;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const USER_AGENT: &str = concat!("uptime-monitoring/", env!("CARGO_PKG_VERSION"));
const MESSAGE_MAX_CHARS: usize = 300;

/// The body is read only when a keyword is set, and only this far.
pub const KEYWORD_SCAN_LIMIT: usize = 1024 * 1024;

/// Holds one shared client per `(follow_redirects, ignore_tls_errors)` pair, built on first use:
/// those two settings can only be set when a client is built.
pub struct HttpChecker {
    guard: Guard,
    clients: [OnceLock<Client>; 4],
}

impl HttpChecker {
    pub fn new(guard: Guard) -> Self {
        // Fails only when a provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self {
            guard,
            clients: Default::default(),
        }
    }

    /// Never fails: every problem becomes the outcome's `failure`. The whole check, body
    /// included, is bounded by the monitor's timeout.
    pub async fn check(&self, monitor: &Monitor) -> CheckOutcome {
        let mut outcome = CheckOutcome {
            checked_at: now_ms(),
            latency_ms: None,
            status_code: None,
            failure: None,
            tls_expires_at: None,
        };
        let limit = Duration::from_secs(monitor.timeout_s.max(1).unsigned_abs());
        let result = tokio::time::timeout(limit, self.run(monitor, &mut outcome)).await;
        outcome.failure = match result {
            Ok(Ok(())) => None,
            Ok(Err(failure)) => Some(failure),
            Err(_) => Some(failure(
                ErrorKind::Timeout,
                format!("no complete response within {} s", monitor.timeout_s),
            )),
        };
        outcome
    }

    async fn run(&self, monitor: &Monitor, outcome: &mut CheckOutcome) -> Result<(), Failure> {
        let started = Instant::now();
        let url = Url::parse(&monitor.url)
            .map_err(|_| failure(ErrorKind::Connect, "invalid URL".to_string()))?;
        self.guard.check_url(&url).map_err(|e| guard_failure(&e))?;
        let method = Method::from_bytes(monitor.method.as_bytes())
            .map_err(|_| failure(ErrorKind::Connect, "invalid HTTP method".to_string()))?;

        let mut request = self
            .client(monitor.follow_redirects, monitor.ignore_tls_errors)
            .request(method, url);
        if let Some(headers) = &monitor.headers {
            let headers: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(headers).unwrap_or_default();
            for (name, value) in headers {
                if let Some(value) = value.as_str() {
                    request = request.header(name, value);
                }
            }
        }
        if let Some(body) = &monitor.body {
            request = request.body(body.clone());
        }

        let mut response = request.send().await.map_err(|e| classify(&e))?;
        outcome.latency_ms = Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX));
        let status = response.status().as_u16();
        outcome.status_code = Some(status);
        outcome.tls_expires_at = response
            .extensions()
            .get::<reqwest::tls::TlsInfo>()
            .and_then(|info| info.peer_certificate())
            .and_then(certificate_expiry_ms);

        if !status_accepted(&monitor.accepted_statuses, status) {
            return Err(failure(
                ErrorKind::Status,
                format!("HTTP {status}, expected {}", monitor.accepted_statuses),
            ));
        }

        let Some(keyword) = &monitor.keyword else {
            return Ok(());
        };
        let mut scan = KeywordScan::new(keyword);
        let mut read = 0;
        while read < KEYWORD_SCAN_LIMIT {
            let Some(chunk) = response.chunk().await.map_err(|e| classify(&e))? else {
                break;
            };
            let take = chunk.len().min(KEYWORD_SCAN_LIMIT - read);
            read += take;
            if scan.feed(&chunk[..take]) {
                return Ok(());
            }
        }
        Err(failure(
            ErrorKind::Keyword,
            format!("keyword {keyword:?} not found in the response"),
        ))
    }

    fn client(&self, follow_redirects: bool, ignore_tls_errors: bool) -> &Client {
        let slot = usize::from(follow_redirects) * 2 + usize::from(ignore_tls_errors);
        self.clients[slot].get_or_init(|| {
            Client::builder()
                .user_agent(USER_AGENT)
                // A proxy would resolve names itself, out of the guard's reach.
                .no_proxy()
                // A fresh connection per check: real DNS + TCP + TLS, no idle socket kept.
                .pool_max_idle_per_host(0)
                .tls_info(true)
                .tls_danger_accept_invalid_certs(ignore_tls_errors)
                .redirect(self.guard.redirect_policy(follow_redirects))
                .dns_resolver(self.guard.clone())
                .build()
                .expect("static HTTP client configuration")
        })
    }
}

fn failure(kind: ErrorKind, message: String) -> Failure {
    let message = if message.chars().count() > MESSAGE_MAX_CHARS {
        message
            .chars()
            .take(MESSAGE_MAX_CHARS - 1)
            .chain(['…'])
            .collect()
    } else {
        message
    };
    Failure { kind, message }
}

fn guard_failure(error: &GuardError) -> Failure {
    let kind = match error {
        GuardError::Dns(_) => ErrorKind::Dns,
        GuardError::Blocked(_) => ErrorKind::Blocked,
        GuardError::TooManyRedirects => ErrorKind::Connect,
    };
    failure(kind, error.to_string())
}

/// Walks the error's source chain: our guard's errors and TLS errors are recognized wherever
/// they sit, including inside an `io::Error` (whose `source()` skips the wrapped error).
fn classify(error: &reqwest::Error) -> Failure {
    let mut current: Option<&(dyn Error + 'static)> = Some(error);
    let mut root: &(dyn Error + 'static) = error;
    while let Some(e) = current {
        root = e;
        if let Some(guard) = e.downcast_ref::<GuardError>() {
            return guard_failure(guard);
        }
        if let Some(tls) = e.downcast_ref::<rustls::Error>() {
            return failure(ErrorKind::Tls, tls_message(tls));
        }
        current = match e
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.get_ref())
        {
            Some(inner) => Some(inner as &(dyn Error + 'static)),
            None => e.source(),
        };
    }
    if error.is_timeout() {
        return failure(ErrorKind::Timeout, "request timed out".to_string());
    }
    failure(ErrorKind::Connect, format!("connection failed: {root}"))
}

/// The common certificate problems in plain words; rustls's own text for the rest.
fn tls_message(error: &rustls::Error) -> String {
    use rustls::CertificateError as C;
    let reason = match error {
        rustls::Error::InvalidCertificate(certificate) => match certificate {
            C::Expired | C::ExpiredContext { .. } => "certificate expired".to_string(),
            C::NotValidYet | C::NotValidYetContext { .. } => {
                "certificate not valid yet".to_string()
            }
            C::UnknownIssuer => "certificate issued by an unknown authority".to_string(),
            C::NotValidForName | C::NotValidForNameContext { .. } => {
                "certificate not valid for this host name".to_string()
            }
            C::Revoked => "certificate revoked".to_string(),
            C::Other(other) => format!("untrusted certificate ({other})"),
            other => format!("invalid certificate ({other})"),
        },
        other => other.to_string(),
    };
    format!("TLS error: {reason}")
}

fn status_accepted(accepted: &str, status: u16) -> bool {
    parse_status_ranges(accepted)
        .map(|ranges| {
            ranges
                .iter()
                .any(|&(low, high)| (low..=high).contains(&status))
        })
        .unwrap_or(false)
}

/// Searches a keyword across body chunks without keeping the whole body.
pub struct KeywordScan<'a> {
    keyword: &'a [u8],
    tail: Vec<u8>,
}

impl<'a> KeywordScan<'a> {
    pub fn new(keyword: &'a str) -> Self {
        Self {
            keyword: keyword.as_bytes(),
            tail: Vec::new(),
        }
    }

    /// Returns `true` once the keyword has been seen.
    pub fn feed(&mut self, chunk: &[u8]) -> bool {
        if self.keyword.is_empty() {
            return true;
        }
        self.tail.extend_from_slice(chunk);
        if self
            .tail
            .windows(self.keyword.len())
            .any(|window| window == self.keyword)
        {
            return true;
        }
        // Keep just enough to match a keyword split across this chunk and the next.
        let keep = self.keyword.len() - 1;
        let drop = self.tail.len().saturating_sub(keep);
        self.tail.drain(..drop);
        false
    }
}

/// `notAfter` of a DER certificate, in epoch milliseconds.
pub fn certificate_expiry_ms(der: &[u8]) -> Option<i64> {
    let (_, certificate) = x509_parser::parse_x509_certificate(der).ok()?;
    certificate
        .validity()
        .not_after
        .timestamp()
        .checked_mul(1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_in_one_chunk() {
        assert!(KeywordScan::new("world").feed(b"hello world"));
        assert!(!KeywordScan::new("absent").feed(b"hello world"));
    }

    #[test]
    fn keyword_across_chunks() {
        let mut scan = KeywordScan::new("needle");
        assert!(!scan.feed(b"hay hay ne"));
        assert!(!scan.feed(b"e"));
        assert!(scan.feed(b"dle hay"));
    }

    #[test]
    fn keyword_tail_stays_small() {
        let mut scan = KeywordScan::new("abc");
        for _ in 0..1000 {
            assert!(!scan.feed(&[b'x'; 4096]));
        }
        assert!(scan.tail.len() < 3);
    }

    #[test]
    fn tls_messages_are_readable() {
        use rustls::CertificateError as C;
        let message = |e: C| tls_message(&rustls::Error::InvalidCertificate(e));
        assert_eq!(message(C::Expired), "TLS error: certificate expired");
        assert_eq!(
            message(C::UnknownIssuer),
            "TLS error: certificate issued by an unknown authority"
        );
        assert_eq!(
            message(C::NotValidForName),
            "TLS error: certificate not valid for this host name"
        );
        assert_eq!(
            tls_message(&rustls::Error::DecryptError),
            "TLS error: cannot decrypt peer's message"
        );
    }

    #[test]
    fn certificate_expiry() {
        let der = include_bytes!("../../tests/fixtures/cert.der");
        assert_eq!(certificate_expiry_ms(der), Some(1_924_991_999_000));
        assert_eq!(certificate_expiry_ms(b"not a certificate"), None);
    }
}
