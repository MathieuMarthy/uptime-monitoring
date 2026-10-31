//! Logins whose password was right and that wait for their two-factor code. In memory: a
//! restart only sends the admin back to the password step.

use crate::auth::{random_token, sha256_hex};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const LOGIN_COOKIE: &str = "__Host-login";
const TTL: Duration = Duration::from_secs(5 * 60);
/// Only someone with the password creates entries; the bound keeps a leaked password from
/// filling memory.
const MAX_ENTRIES: usize = 100;
const MAX_FAILURES: u8 = 5;

#[derive(Default)]
pub struct PendingLogins {
    /// Keyed by the SHA-256 of the cookie token.
    entries: HashMap<String, Pending>,
}

struct Pending {
    /// The hash the password was checked against: a password change meanwhile voids the entry.
    password_hash: String,
    expires: Instant,
    failures: u8,
}

impl PendingLogins {
    /// A new pending login and its cookie token; `None` when the table is full.
    pub fn start(&mut self, password_hash: String, now: Instant) -> Option<String> {
        self.entries.retain(|_, pending| pending.expires > now);
        if self.entries.len() >= MAX_ENTRIES {
            return None;
        }
        let token = random_token();
        self.entries.insert(
            sha256_hex(&token),
            Pending {
                password_hash,
                expires: now + TTL,
                failures: 0,
            },
        );
        Some(token)
    }

    /// The password hash of the unexpired pending login of `token`.
    pub fn password_hash(&self, token: &str, now: Instant) -> Option<&str> {
        self.entries
            .get(&sha256_hex(token))
            .filter(|pending| pending.expires > now)
            .map(|pending| pending.password_hash.as_str())
    }

    /// Counts a wrong code; `false` when that ended the pending login.
    pub fn fail(&mut self, token: &str) -> bool {
        let key = sha256_hex(token);
        let Some(pending) = self.entries.get_mut(&key) else {
            return false;
        };
        pending.failures += 1;
        if pending.failures >= MAX_FAILURES {
            self.entries.remove(&key);
            return false;
        }
        true
    }

    pub fn finish(&mut self, token: &str) {
        self.entries.remove(&sha256_hex(token));
    }
}

pub fn login_cookie(token: &str) -> String {
    let max_age = TTL.as_secs();
    format!("{LOGIN_COOKIE}={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={max_age}")
}

pub fn expired_login_cookie() -> String {
    format!("{LOGIN_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_expire_and_end_after_five_failures() {
        let mut logins = PendingLogins::default();
        let now = Instant::now();
        let token = logins.start("hash".to_string(), now).unwrap();
        assert_eq!(logins.password_hash(&token, now), Some("hash"));
        assert_eq!(logins.password_hash("other", now), None);
        assert_eq!(logins.password_hash(&token, now + TTL), None);
        for _ in 0..4 {
            assert!(logins.fail(&token));
        }
        assert!(!logins.fail(&token));
        assert_eq!(logins.password_hash(&token, now), None);
    }

    #[test]
    fn table_is_bounded_and_swept() {
        let mut logins = PendingLogins::default();
        let now = Instant::now();
        for _ in 0..MAX_ENTRIES {
            logins.start("hash".to_string(), now).unwrap();
        }
        assert!(logins.start("hash".to_string(), now).is_none());
        assert!(logins.start("hash".to_string(), now + TTL).is_some());
        assert_eq!(logins.entries.len(), 1);
    }
}
