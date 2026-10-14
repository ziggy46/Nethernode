//! Time-based one-time passwords (RFC 6238, the kind authenticator apps
//! show) and recovery codes, for two-factor sign-in.

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, KeyInit, Mac};
use rand::{Rng, RngExt};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Seconds per code.
pub const PERIOD: i64 = 30;
/// Digits per code.
pub const DIGITS: u32 = 6;
/// Secret length; 160 bits, as RFC 4226 recommends.
pub const SECRET_BYTES: usize = 20;
/// Codes this many steps either side of now still count, for clock drift.
pub const SKEW: i64 = 1;
/// Recovery codes issued at enrollment.
pub const RECOVERY_CODES: usize = 10;

/// The issuer shown in authenticator apps.
const ISSUER: &str = "NetherNode";

/// A new random secret, base32 encoded for typing into an app.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    BASE32_NOPAD.encode(&bytes)
}

/// The time step containing `unix_secs`.
pub fn step_at(unix_secs: i64) -> i64 {
    unix_secs.div_euclid(PERIOD)
}

/// The HOTP value for `step` (RFC 4226 dynamic truncation).
pub fn code_at(secret: &[u8], step: i64, digits: u32) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(&step.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = usize::from(hash[hash.len() - 1] & 0x0f);
    let value = u32::from_be_bytes([
        hash[offset] & 0x7f,
        hash[offset + 1],
        hash[offset + 2],
        hash[offset + 3],
    ]);
    value % 10u32.pow(digits)
}

/// Decodes a base32 secret, tolerating spaces and lowercase.
pub fn decode_secret(secret: &str) -> Option<Vec<u8>> {
    let cleaned: String = secret
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    BASE32_NOPAD.decode(cleaned.as_bytes()).ok()
}

/// Checks a 6-digit `code` against the steps around `unix_secs`. Returns
/// the step it matched, so the caller can refuse it if it's used again.
pub fn verify(secret: &str, code: &str, unix_secs: i64) -> Option<i64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let key = decode_secret(secret)?;
    let now = step_at(unix_secs);
    // Check every candidate so timing doesn't reveal which one matched.
    let mut matched = None;
    for step in (now - SKEW)..=(now + SKEW) {
        let expected = format!(
            "{:0width$}",
            code_at(&key, step, DIGITS),
            width = DIGITS as usize
        );
        if bool::from(expected.as_bytes().ct_eq(code.as_bytes())) && matched.is_none() {
            matched = Some(step);
        }
    }
    matched
}

/// The `otpauth://` link an authenticator app reads from the QR code.
pub fn otpauth_uri(secret: &str, username: &str) -> String {
    let label: String = username
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!(
        "otpauth://totp/{ISSUER}:{label}?secret={secret}&issuer={ISSUER}&algorithm=SHA1&digits={DIGITS}&period={PERIOD}"
    )
}

/// `data` as a QR code, in SVG.
pub fn qr_svg(data: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(data, qrcode::EcLevel::M).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color<'_>>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .dark_color(qrcode::render::svg::Color("#000000"))
            .light_color(qrcode::render::svg::Color("#ffffff"))
            .build(),
    )
}

/// Letters and digits that can't be mistaken for each other.
const RECOVERY_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Fresh single-use recovery codes, like `k3m9p-x2r7t`.
pub fn generate_recovery_codes() -> Vec<String> {
    let mut rng = rand::rng();
    (0..RECOVERY_CODES)
        .map(|_| {
            let mut code = String::with_capacity(11);
            for i in 0..10 {
                if i == 5 {
                    code.push('-');
                }
                let idx = rng.random_range(0..RECOVERY_ALPHABET.len());
                code.push(char::from(RECOVERY_ALPHABET[idx]));
            }
            code
        })
        .collect()
}

/// Whether `input` looks like a recovery code rather than an app code.
pub fn is_recovery_code(input: &str) -> bool {
    normalize_recovery(input).len() == 10
}

fn normalize_recovery(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// What's stored for a recovery code: SHA-256 of its normalized form.
pub fn hash_recovery_code(code: &str) -> String {
    hex::encode(Sha256::digest(normalize_recovery(code).as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 appendix B, SHA-1, with the 20-byte ASCII secret.
    const VECTORS: [(i64, u32); 6] = [
        (59, 94_287_082),
        (1_111_111_109, 7_081_804),
        (1_111_111_111, 14_050_471),
        (1_234_567_890, 89_005_924),
        (2_000_000_000, 69_279_037),
        (20_000_000_000, 65_353_130),
    ];

    #[test]
    fn rfc_6238_vectors() {
        let key = b"12345678901234567890";
        for (time, expected) in VECTORS {
            assert_eq!(code_at(key, step_at(time), 8), expected, "t={time}");
            assert_eq!(
                code_at(key, step_at(time), 6),
                expected % 1_000_000,
                "t={time}"
            );
        }
    }

    #[test]
    fn verify_accepts_neighbouring_steps_only() {
        let secret = BASE32_NOPAD.encode(b"12345678901234567890");
        assert_eq!(verify(&secret, "287082", 59), Some(1));
        // 30 s later the old code is still accepted (one step of drift)…
        assert_eq!(verify(&secret, "287082", 89), Some(1));
        // …but not 90 s later.
        assert_eq!(verify(&secret, "287082", 119), None);
        assert_eq!(verify(&secret, "287 082", 59), Some(1));
        assert_eq!(verify(&secret, "28708", 59), None);
        assert_eq!(verify(&secret, "28708a", 59), None);
        assert_eq!(verify("not base32!", "287082", 59), None);
    }

    #[test]
    fn secrets_round_trip() {
        let secret = generate_secret();
        assert_eq!(secret.len(), 32);
        assert_eq!(
            decode_secret(&secret.to_lowercase()).unwrap().len(),
            SECRET_BYTES
        );
        let uri = otpauth_uri(&secret, "Steve.B");
        assert!(uri.starts_with("otpauth://totp/NetherNode:Steve.B?secret="));
        assert!(qr_svg(&uri).unwrap().starts_with("<?xml"));
    }

    #[test]
    fn recovery_codes() {
        let codes = generate_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODES);
        let first = &codes[0];
        assert_eq!(first.len(), 11);
        assert!(is_recovery_code(first) && !is_recovery_code("123456"));
        assert_eq!(
            hash_recovery_code(first),
            hash_recovery_code(&first.to_uppercase().replace('-', " "))
        );
    }
}
