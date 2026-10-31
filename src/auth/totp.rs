//! Time-based one-time passwords (RFC 6238): HMAC-SHA1, 6 digits, 30 s steps, as every
//! authenticator app expects by default.

use hmac::{Hmac, KeyInit, Mac};
use qrcode::{Color, QrCode};
use sha1::Sha1;

const STEP_MS: i64 = 30_000;
const DIGITS: u32 = 6;
const SECRET_BYTES: usize = 20;
/// Steps accepted on each side of the current one, for clock drift and typing time.
const DRIFT_STEPS: i64 = 1;
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A new random secret, base32-encoded as authenticator apps take it.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    getrandom::fill(&mut bytes).expect("the OS random number generator is unavailable");
    base32_encode(&bytes)
}

/// The secret's bytes when `secret` is a base32 secret of the size `generate_secret` makes.
pub fn decode_secret(secret: &str) -> Option<Vec<u8>> {
    base32_decode(secret).filter(|bytes| bytes.len() == SECRET_BYTES)
}

/// The code of a time step (`unix_ms / 30 000`).
pub fn code_at(secret: &[u8], step: i64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC takes keys of any size");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let truncated =
        u32::from_be_bytes(digest[offset..offset + 4].try_into().unwrap()) & 0x7fff_ffff;
    truncated % 10u32.pow(DIGITS)
}

pub fn step_at(now_ms: i64) -> i64 {
    now_ms.div_euclid(STEP_MS)
}

/// The step `code` belongs to, within the drift window around `now_ms` and later than
/// `last_step` (a code is used once). Spaces in the code are ignored.
pub fn verify(secret: &[u8], code: &str, now_ms: i64, last_step: Option<i64>) -> Option<i64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code: u32 = code.parse().ok()?;
    let now = step_at(now_ms);
    (now - DRIFT_STEPS..=now + DRIFT_STEPS)
        .filter(|step| last_step.is_none_or(|last| *step > last))
        .find(|step| code_at(secret, *step) == code)
}

/// The `otpauth://` URI authenticator apps read from the QR code.
pub fn otpauth_uri(secret: &str, username: &str) -> String {
    // `byte_serialize` turns spaces into `+`, which a URI path keeps as is.
    let label = form_urlencoded::byte_serialize(username.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    format!("otpauth://totp/Uptime%20Monitoring:{label}?secret={secret}&issuer=Uptime%20Monitoring")
}

/// The QR code of `data` as an SVG element: one path, one unit per module, with the quiet zone.
pub fn qr_svg(data: &str) -> String {
    let code = QrCode::new(data.as_bytes()).expect("an otpauth URI fits in a QR code");
    let width = code.width();
    let size = width + 8;
    let mut path = String::new();
    for (i, color) in code.to_colors().iter().enumerate() {
        if *color == Color::Dark {
            path.push_str(&format!("M{},{}h1v1h-1z", i % width + 4, i / width + 4));
        }
    }
    format!(
        "<svg class=\"qr-code\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {size} {size}\" \
         role=\"img\" aria-label=\"QR code of the secret\"><path d=\"{path}\"/></svg>"
    )
}

fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Unpadded base32, case-insensitive. Trailing bits that do not make a byte are dropped.
fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let value = BASE32.iter().position(|b| *b == c.to_ascii_uppercase())?;
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn codes_match_rfc_6238() {
        // The RFC's SHA-1 vectors, last 6 of their 8 digits.
        for (seconds, code) in [
            (59, 287_082),
            (1_111_111_109, 81_804),
            (1_234_567_890, 5_924),
            (2_000_000_000, 279_037),
        ] {
            assert_eq!(
                code_at(RFC_SECRET, step_at(seconds * 1000)),
                code,
                "{seconds}"
            );
        }
    }

    #[test]
    fn base32_matches_rfc_4648() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "MY"),
            ("foo", "MZXW6"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(base32_encode(plain.as_bytes()), encoded);
            assert_eq!(base32_decode(encoded).unwrap(), plain.as_bytes());
        }
        assert_eq!(base32_decode("mzxw6").unwrap(), b"foo");
        assert_eq!(base32_decode("MZ1"), None);
    }

    #[test]
    fn secrets_round_trip() {
        let secret = generate_secret();
        assert_eq!(secret.len(), 32);
        assert_eq!(decode_secret(&secret).unwrap().len(), SECRET_BYTES);
        assert_ne!(secret, generate_secret());
        assert_eq!(decode_secret("MZXW6"), None);
    }

    #[test]
    fn verify_accepts_one_step_of_drift_and_each_step_once() {
        let now = 1_234_567_890_000;
        let step = step_at(now);
        let code = |s| format!("{:06}", code_at(RFC_SECRET, s));
        assert_eq!(verify(RFC_SECRET, &code(step), now, None), Some(step));
        assert_eq!(
            verify(RFC_SECRET, &code(step - 1), now, None),
            Some(step - 1)
        );
        assert_eq!(
            verify(RFC_SECRET, &code(step + 1), now, None),
            Some(step + 1)
        );
        assert_eq!(verify(RFC_SECRET, &code(step - 2), now, None), None);
        assert_eq!(verify(RFC_SECRET, &code(step), now, Some(step)), None);
        assert_eq!(
            verify(RFC_SECRET, &code(step), now, Some(step - 1)),
            Some(step)
        );
        let spaced = format!("{} {}", &code(step)[..3], &code(step)[3..]);
        assert_eq!(verify(RFC_SECRET, &spaced, now, None), Some(step));
        assert_eq!(verify(RFC_SECRET, "12345", now, None), None);
        assert_eq!(verify(RFC_SECRET, "12a456", now, None), None);
    }

    #[test]
    fn uri_and_qr_code() {
        assert_eq!(
            otpauth_uri("ABC", "jo doe"),
            "otpauth://totp/Uptime%20Monitoring:jo%20doe?secret=ABC&issuer=Uptime%20Monitoring"
        );
        let svg = qr_svg(&otpauth_uri(&generate_secret(), "admin"));
        assert!(svg.starts_with("<svg class=\"qr-code\""));
        assert!(svg.contains("<path d=\"M4,4h1v1h-1z"));
    }
}
