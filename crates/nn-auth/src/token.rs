//! Session and CSRF tokens.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Number of random bytes in a session token.
pub const TOKEN_BYTES: usize = 32;

/// Generates a URL-safe token from 32 random bytes (43 characters).
pub fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Hex-encoded SHA-256 of a token. This, not the token, is what gets stored.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Compares two secrets in constant time.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Generates a random password for account recovery: 24 URL-safe characters.
pub fn generate_password() -> String {
    let mut bytes = [0u8; 18];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_32_bytes_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(URL_SAFE_NO_PAD.decode(&a).unwrap().len(), TOKEN_BYTES);
    }

    #[test]
    fn token_hash_is_sha256_hex() {
        // SHA-256("abc")
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn constant_time_comparison() {
        assert!(constant_time_eq("same", "same"));
        assert!(!constant_time_eq("same", "diff"));
        assert!(!constant_time_eq("same", "samesame"));
    }

    #[test]
    fn generated_passwords_meet_length_rules() {
        let password = generate_password();
        assert_eq!(password.len(), 24);
        assert!(crate::password::validate_password(&password).is_ok());
    }
}
