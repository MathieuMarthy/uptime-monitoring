//! Password hashing, random tokens and the session cookie.

pub mod rate_limit;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::http::HeaderMap;
use axum::http::header;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const SESSION_COOKIE: &str = "__Host-session";
pub const SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Prints the first-launch URL to stderr, whatever `RUST_LOG` filters out.
pub fn print_setup_url(token: &str) {
    eprintln!("No admin account yet. Create it by opening /setup?token={token} on this instance.");
}

/// Hashes a password with Argon2id and the crate's default (OWASP) parameters. CPU and memory
/// heavy: call it from a blocking task.
pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    Ok(Argon2::default()
        .hash_password(password.as_bytes())?
        .to_string())
}

/// Whether `password` matches the PHC string `hash`; a malformed hash never matches.
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

/// 32 random bytes from the OS, hex-encoded.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random number generator is unavailable");
    hex(&bytes)
}

pub fn sha256_hex(value: &str) -> String {
    hex(&Sha256::digest(value.as_bytes()))
}

/// Compares two secrets in constant time.
pub fn secrets_match(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The value of the named cookie in the request headers.
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

pub fn session_cookie(token: &str) -> String {
    let max_age = SESSION_TTL_MS / 1000;
    format!("{SESSION_COOKIE}={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={max_age}")
}

pub fn expired_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn password_round_trip() {
        let hash = hash_password("a long password").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("a long password", &hash));
        assert!(!verify_password("another password", &hash));
        assert!(!verify_password("a long password", "not-a-hash"));
    }

    #[test]
    fn tokens_are_random_hex() {
        let token = random_token();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(token, random_token());
    }

    #[test]
    fn sha256_is_hex_digest() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn cookie_is_found_among_others() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("a=1; __Host-session=abc"),
        );
        headers.append(header::COOKIE, HeaderValue::from_static("b=2"));
        assert_eq!(cookie(&headers, SESSION_COOKIE), Some("abc"));
        assert_eq!(cookie(&headers, "b"), Some("2"));
        assert_eq!(cookie(&headers, "session"), None);
    }
}
