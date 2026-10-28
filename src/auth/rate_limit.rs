//! In-memory limit on failed login attempts, per client IP.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

const MAX_FAILURES: u32 = 10;
const WINDOW: Duration = Duration::from_secs(15 * 60);
/// Bounds memory. When every slot holds a live entry, unknown IPs are refused until some expire.
const MAX_ENTRIES: usize = 10_000;

#[derive(Default)]
pub struct LoginLimiter {
    entries: HashMap<IpAddr, Entry>,
}

struct Entry {
    failures: u32,
    window_start: Instant,
}

impl Entry {
    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.window_start) >= WINDOW
    }
}

impl LoginLimiter {
    pub fn is_blocked(&mut self, ip: IpAddr, now: Instant) -> bool {
        if let Some(entry) = self.entries.get(&ip) {
            if entry.expired(now) {
                self.entries.remove(&ip);
                return false;
            }
            return entry.failures >= MAX_FAILURES;
        }
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.retain(|_, entry| !entry.expired(now));
        }
        self.entries.len() >= MAX_ENTRIES
    }

    pub fn record_failure(&mut self, ip: IpAddr, now: Instant) {
        let entry = self.entries.entry(ip).or_insert(Entry {
            failures: 0,
            window_start: now,
        });
        if entry.expired(now) {
            *entry = Entry {
                failures: 0,
                window_start: now,
            };
        }
        entry.failures += 1;
    }

    pub fn record_success(&mut self, ip: IpAddr) {
        self.entries.remove(&ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(n: u32) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from(n))
    }

    #[test]
    fn blocks_after_max_failures_until_window_ends() {
        let mut limiter = LoginLimiter::default();
        let start = Instant::now();
        for _ in 0..MAX_FAILURES {
            assert!(!limiter.is_blocked(ip(1), start));
            limiter.record_failure(ip(1), start);
        }
        assert!(limiter.is_blocked(ip(1), start + WINDOW - Duration::from_secs(1)));
        assert!(!limiter.is_blocked(ip(2), start));
        assert!(!limiter.is_blocked(ip(1), start + WINDOW));
    }

    #[test]
    fn success_clears_failures() {
        let mut limiter = LoginLimiter::default();
        let now = Instant::now();
        for _ in 0..MAX_FAILURES {
            limiter.record_failure(ip(1), now);
        }
        limiter.record_success(ip(1));
        assert!(!limiter.is_blocked(ip(1), now));
    }

    #[test]
    fn full_table_refuses_new_ips_until_entries_expire() {
        let mut limiter = LoginLimiter::default();
        let start = Instant::now();
        for n in 0..MAX_ENTRIES as u32 {
            limiter.record_failure(ip(n), start);
        }
        let newcomer = ip(MAX_ENTRIES as u32);
        assert!(limiter.is_blocked(newcomer, start));
        assert!(!limiter.is_blocked(ip(0), start));
        assert!(!limiter.is_blocked(newcomer, start + WINDOW));
        assert!(limiter.entries.is_empty());
    }
}
