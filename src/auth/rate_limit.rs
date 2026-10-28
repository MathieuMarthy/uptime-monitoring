//! In-memory limit on login attempts, per client network.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

const MAX_FAILURES: u32 = 10;
const WINDOW: Duration = Duration::from_secs(15 * 60);
/// Bounds memory. When every slot holds a live entry, unknown clients are refused until some
/// expire: forgetting an attacker's entry would hand it a fresh budget.
const MAX_ENTRIES: usize = 10_000;
/// A full table is swept for expired entries at most this often.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct LoginLimiter {
    entries: HashMap<IpAddr, Entry>,
    last_sweep: Option<Instant>,
}

struct Entry {
    attempts: u32,
    window_start: Instant,
}

impl Entry {
    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.window_start) >= WINDOW
    }
}

impl LoginLimiter {
    /// Counts a login attempt before the password is checked, so concurrent requests cannot
    /// all pass. Returns false when the client is over its budget.
    pub fn try_attempt(&mut self, ip: IpAddr, now: Instant) -> bool {
        let key = client_key(ip);
        if !self.entries.contains_key(&key) && self.entries.len() >= MAX_ENTRIES {
            self.sweep(now);
            if self.entries.len() >= MAX_ENTRIES {
                return false;
            }
        }
        let entry = self.entries.entry(key).or_insert(Entry {
            attempts: 0,
            window_start: now,
        });
        if entry.expired(now) {
            *entry = Entry {
                attempts: 0,
                window_start: now,
            };
        }
        if entry.attempts >= MAX_FAILURES {
            return false;
        }
        entry.attempts += 1;
        true
    }

    /// A successful login clears the client's count.
    pub fn record_success(&mut self, ip: IpAddr) {
        self.entries.remove(&client_key(ip));
    }

    fn sweep(&mut self, now: Instant) {
        if self
            .last_sweep
            .is_some_and(|last| now.duration_since(last) < SWEEP_INTERVAL)
        {
            return;
        }
        self.last_sweep = Some(now);
        self.entries.retain(|_, entry| !entry.expired(now));
    }
}

/// IPv6 clients usually own a whole /64: they share one budget.
fn client_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6((u128::from(v6) & !u128::from(u64::MAX)).into()),
        },
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
    fn attempts_are_counted_before_the_outcome() {
        let mut limiter = LoginLimiter::default();
        let start = Instant::now();
        for _ in 0..MAX_FAILURES {
            assert!(limiter.try_attempt(ip(1), start));
        }
        assert!(!limiter.try_attempt(ip(1), start + WINDOW - Duration::from_secs(1)));
        assert!(limiter.try_attempt(ip(2), start));
        assert!(limiter.try_attempt(ip(1), start + WINDOW));
    }

    #[test]
    fn success_clears_attempts() {
        let mut limiter = LoginLimiter::default();
        let now = Instant::now();
        for _ in 0..MAX_FAILURES {
            limiter.try_attempt(ip(1), now);
        }
        limiter.record_success(ip(1));
        assert!(limiter.try_attempt(ip(1), now));
    }

    #[test]
    fn ipv6_addresses_share_their_64_prefix() {
        let mut limiter = LoginLimiter::default();
        let now = Instant::now();
        for n in 0..MAX_FAILURES {
            let addr: IpAddr = format!("2001:db8:1:2::{:x}", n + 1).parse().unwrap();
            assert!(limiter.try_attempt(addr, now));
        }
        assert!(!limiter.try_attempt("2001:db8:1:2:ffff::1".parse().unwrap(), now));
        assert!(limiter.try_attempt("2001:db8:1:3::1".parse().unwrap(), now));
        assert_eq!(
            client_key("::ffff:10.0.0.1".parse().unwrap()),
            "10.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn full_table_refuses_new_clients_until_entries_expire() {
        let mut limiter = LoginLimiter::default();
        let start = Instant::now();
        for n in 0..MAX_ENTRIES as u32 {
            limiter.try_attempt(ip(n), start);
        }
        let newcomer = ip(MAX_ENTRIES as u32);
        assert!(!limiter.try_attempt(newcomer, start));
        assert!(limiter.try_attempt(ip(0), start));
        assert!(limiter.try_attempt(newcomer, start + WINDOW));
        assert_eq!(limiter.entries.len(), 1);
    }

    #[test]
    fn full_table_is_swept_at_most_once_per_interval() {
        let mut limiter = LoginLimiter::default();
        let start = Instant::now();
        for n in 0..MAX_ENTRIES as u32 {
            limiter.try_attempt(ip(n), start);
        }
        let later = start + WINDOW - Duration::from_secs(30);
        assert!(!limiter.try_attempt(ip(u32::MAX), later));
        // Entries expired since the last sweep stay until the next interval.
        assert!(!limiter.try_attempt(ip(u32::MAX), start + WINDOW));
        assert!(limiter.try_attempt(ip(u32::MAX), later + SWEEP_INTERVAL));
    }
}
