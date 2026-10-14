//! Encryption at rest for secrets NetherNode must be able to read back
//! (e.g. per-server RCON passwords), keyed by `data/secret.key`.
//!
//! Sealed values look like `v1:<base64url(nonce ‖ ciphertext)>`, using
//! ChaCha20-Poly1305 with a random 96-bit nonce per value.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::Rng;

const PREFIX: &str = "v1:";
const NONCE_BYTES: usize = 12;

/// Errors from sealing or opening secrets.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The key file is missing, unreadable, or not 64 hex characters.
    #[error("invalid secret key: {0}")]
    InvalidKey(String),
    /// The sealed value is malformed, or was sealed with a different key.
    #[error("could not decrypt secret")]
    Decrypt,
}

/// Seals and opens secrets with the instance key.
pub struct SecretBox {
    cipher: ChaCha20Poly1305,
}

impl std::fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretBox(..)")
    }
}

impl SecretBox {
    /// Builds a box from a 256-bit key given as 64 hex characters.
    pub fn from_hex_key(hex_key: &str) -> Result<Self, SecretError> {
        let bytes =
            hex::decode(hex_key.trim()).map_err(|e| SecretError::InvalidKey(e.to_string()))?;
        if bytes.len() != 32 {
            return Err(SecretError::InvalidKey(format!(
                "expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        let key =
            Key::try_from(bytes.as_slice()).map_err(|e| SecretError::InvalidKey(e.to_string()))?;
        Ok(Self {
            cipher: ChaCha20Poly1305::new(&key),
        })
    }

    /// Loads the key from `path` (the `secret.key` written on first run).
    pub fn load(path: &Path) -> Result<Self, SecretError> {
        let hex_key = std::fs::read_to_string(path)
            .map_err(|e| SecretError::InvalidKey(format!("{}: {e}", path.display())))?;
        Self::from_hex_key(&hex_key)
    }

    /// Encrypts `plaintext`.
    pub fn seal(&self, plaintext: &str) -> String {
        crate::redact::register(plaintext);
        let mut nonce = [0u8; NONCE_BYTES];
        rand::rng().fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(&Nonce::from(nonce), plaintext.as_bytes())
            .expect("ChaCha20-Poly1305 encryption of a short buffer cannot fail");
        let mut out = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(out))
    }

    /// Decrypts a value produced by [`SecretBox::seal`].
    pub fn open(&self, sealed: &str) -> Result<String, SecretError> {
        let encoded = sealed.strip_prefix(PREFIX).ok_or(SecretError::Decrypt)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| SecretError::Decrypt)?;
        if bytes.len() < NONCE_BYTES {
            return Err(SecretError::Decrypt);
        }
        let (nonce, ciphertext) = bytes.split_at(NONCE_BYTES);
        let nonce = Nonce::try_from(nonce).map_err(|_| SecretError::Decrypt)?;
        let plaintext = self
            .cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| SecretError::Decrypt)?;
        let plaintext = String::from_utf8(plaintext).map_err(|_| SecretError::Decrypt)?;
        crate::redact::register(&plaintext);
        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn round_trips_with_fresh_nonces() {
        let sb = SecretBox::from_hex_key(KEY).unwrap();
        let a = sb.seal("rcon-password");
        let b = sb.seal("rcon-password");
        assert_ne!(a, b, "nonce must differ per value");
        assert!(a.starts_with("v1:"));
        assert_eq!(sb.open(&a).unwrap(), "rcon-password");
        assert_eq!(sb.open(&b).unwrap(), "rcon-password");
    }

    #[test]
    fn rejects_tampering_and_wrong_keys() {
        let sb = SecretBox::from_hex_key(KEY).unwrap();
        let sealed = sb.seal("secret");
        let mut tampered = sealed.clone();
        tampered.pop();
        tampered.push(if sealed.ends_with('A') { 'B' } else { 'A' });
        assert!(sb.open(&tampered).is_err());
        let other = SecretBox::from_hex_key(&"ff".repeat(32)).unwrap();
        assert!(other.open(&sealed).is_err());
        assert!(sb.open("plaintext").is_err());
        assert!(sb.open("v1:").is_err());
    }

    #[test]
    fn validates_keys() {
        assert!(SecretBox::from_hex_key("abcd").is_err());
        assert!(SecretBox::from_hex_key("zz").is_err());
        assert!(
            SecretBox::from_hex_key(&format!("{KEY}\n")).is_ok(),
            "trailing newline is fine"
        );
    }
}
