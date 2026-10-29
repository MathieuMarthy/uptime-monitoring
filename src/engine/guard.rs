//! Private-target blocking (`settings.block_private_targets`).
//!
//! Enforced at three points: the DNS resolver drops blocked addresses (the connection then uses
//! exactly the addresses that were checked, so DNS rebinding cannot slip through), a literal IP
//! host is checked before sending, and the redirect policy checks every redirect target.

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::{Url, redirect};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Redirects followed before giving up.
pub const MAX_REDIRECTS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardError {
    /// The name could not be resolved.
    Dns(String),
    /// Every address of the target is a private one, refused by the setting.
    Blocked(String),
    TooManyRedirects,
}

impl std::fmt::Display for GuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Dns(message) => write!(f, "DNS resolution failed: {message}"),
            Self::Blocked(target) => {
                write!(f, "{target} is a private address, blocked by the settings")
            }
            Self::TooManyRedirects => write!(f, "more than {MAX_REDIRECTS} redirects"),
        }
    }
}

impl std::error::Error for GuardError {}

/// Shared by every HTTP client of the engine; the flag can change while running.
#[derive(Clone, Default)]
pub struct Guard {
    block_private: Arc<AtomicBool>,
}

impl Guard {
    pub fn new(block_private: bool) -> Self {
        Self {
            block_private: Arc::new(AtomicBool::new(block_private)),
        }
    }

    pub fn set_block_private(&self, block: bool) {
        self.block_private.store(block, Ordering::Relaxed);
    }

    pub fn blocks_private(&self) -> bool {
        self.block_private.load(Ordering::Relaxed)
    }

    /// Refuses a URL whose host is a literal private IP (those are never resolved).
    pub fn check_url(&self, url: &Url) -> Result<(), GuardError> {
        let ip = match url.host() {
            Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip),
            Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip),
            _ => return Ok(()),
        };
        if self.blocks_private() && is_private(ip) {
            return Err(GuardError::Blocked(ip.to_string()));
        }
        Ok(())
    }

    /// Resolves `host`, keeping only the addresses allowed by the setting.
    pub async fn resolve_host(&self, host: &str) -> Result<Vec<SocketAddr>, GuardError> {
        let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0))
            .await
            .map_err(|e| GuardError::Dns(e.to_string()))?
            .collect();
        if addrs.is_empty() {
            return Err(GuardError::Dns(format!("no address for {host}")));
        }
        if self.blocks_private() {
            addrs.retain(|a| !is_private(a.ip()));
            if addrs.is_empty() {
                return Err(GuardError::Blocked(host.to_string()));
            }
        }
        Ok(addrs)
    }

    /// Follows up to `MAX_REDIRECTS` redirects when `follow` is set, checking each target.
    pub fn redirect_policy(&self, follow: bool) -> redirect::Policy {
        if !follow {
            return redirect::Policy::none();
        }
        let guard = self.clone();
        redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                return attempt.error(GuardError::TooManyRedirects);
            }
            match guard.check_url(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(e) => attempt.error(e),
            }
        })
    }
}

impl Resolve for Guard {
    fn resolve(&self, name: Name) -> Resolving {
        let guard = self.clone();
        Box::pin(async move {
            let addrs = guard.resolve_host(name.as_str()).await?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Loopback, private, link-local (cloud metadata), CGNAT, benchmarking, IETF protocol,
/// multicast, reserved and unspecified addresses. IPv6 addresses embedding an IPv4 one
/// (mapped, compatible, NAT64, 6to4) are judged as that IPv4.
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_private_v4(ip),
        IpAddr::V6(ip) => is_private_v6(ip),
    }
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (b & 0xc0) == 64)
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b & 0xfe) == 18)
        || a >= 240
}

fn is_private_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    if ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first & 0xffc0) == 0xfec0
    {
        return true;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_v4(v4);
    }
    let s = ip.segments();
    let embedded = |high: u16, low: u16| Ipv4Addr::from((u32::from(high) << 16) | u32::from(low));
    // NAT64 (64:ff9b::/96) and 6to4 (2002::/16) reach the IPv4 address they embed.
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_private_v4(embedded(s[6], s[7]));
    }
    if first == 0x2002 {
        return is_private_v4(embedded(s[1], s[2]));
    }
    // IPv4-compatible (`::a.b.c.d`), deprecated but still routed by some stacks.
    if ip.segments()[..6].iter().all(|&s| s == 0) {
        return ip.to_ipv4().is_some_and(is_private_v4);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_addresses() {
        for ip in [
            "127.0.0.1",
            "127.255.0.9",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "0.1.2.3",
            "255.255.255.255",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::10.0.0.1",
            "192.0.0.1",
            "198.18.0.1",
            "198.19.255.255",
            "fec0::1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b::7f00:1",
            "2002:7f00:1::1",
            "2002:a9fe:a9fe::",
        ] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn public_addresses() {
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "172.15.255.255",
            "172.32.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "192.169.0.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
            "198.20.0.1",
            "64:ff9b::808:808",
            "2002:808:808::1",
        ] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn literal_ip_urls() {
        let url = |s: &str| Url::parse(s).unwrap();
        let on = Guard::new(true);
        assert!(matches!(
            on.check_url(&url("http://127.0.0.1:8080/")),
            Err(GuardError::Blocked(_))
        ));
        assert!(on.check_url(&url("http://[::1]/")).is_err());
        assert!(on.check_url(&url("http://[::ffff:a00:1]/")).is_err());
        assert!(on.check_url(&url("https://1.1.1.1/")).is_ok());
        assert!(on.check_url(&url("https://example.com/")).is_ok());
        assert!(
            Guard::new(false)
                .check_url(&url("http://127.0.0.1/"))
                .is_ok()
        );
    }

    #[tokio::test]
    async fn resolver_filters_private_addresses() {
        assert!(matches!(
            Guard::new(true).resolve_host("localhost").await,
            Err(GuardError::Blocked(_))
        ));
        let addrs = Guard::new(false).resolve_host("localhost").await.unwrap();
        assert!(addrs.iter().all(|a| a.ip().is_loopback()));
        assert!(matches!(
            Guard::new(false)
                .resolve_host("does-not-exist.invalid")
                .await,
            Err(GuardError::Dns(_))
        ));
    }

    #[test]
    fn flag_can_change_at_runtime() {
        let guard = Guard::new(false);
        let url = Url::parse("http://10.0.0.1/").unwrap();
        assert!(guard.check_url(&url).is_ok());
        guard.set_block_private(true);
        assert!(guard.check_url(&url).is_err());
    }
}
