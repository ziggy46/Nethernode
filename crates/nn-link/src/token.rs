//! Join tokens: what an agent needs to join a panel the first time.
//!
//! `nnj1.<base64url>` of the code ID (10 bytes), the code's secret (16
//! bytes), and the panel's public key (32 bytes). The panel stores only the
//! secret's SHA-256; the agent pins the panel key from the token.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};

const PREFIX: &str = "nnj1.";

/// A one-time code to join a panel.
#[derive(Clone, PartialEq, Eq)]
pub struct JoinToken {
    /// Identifies the code (not secret).
    pub code_id: String,
    /// Proves the agent was given the code.
    pub secret: [u8; 16],
    /// The panel's Noise public key.
    pub panel_key: [u8; 32],
}

impl std::fmt::Debug for JoinToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinToken")
            .field("code_id", &self.code_id)
            .finish_non_exhaustive()
    }
}

impl JoinToken {
    /// A new random code for joining the panel with `panel_key`.
    pub fn generate(panel_key: [u8; 32]) -> Self {
        let mut id = [0u8; 10];
        let mut secret = [0u8; 16];
        rand::rng().fill_bytes(&mut id);
        rand::rng().fill_bytes(&mut secret);
        Self {
            code_id: hex::encode(id),
            secret,
            panel_key,
        }
    }

    /// The token as text, for the join command.
    pub fn encode(&self) -> String {
        let mut raw = Vec::with_capacity(58);
        raw.extend_from_slice(&hex::decode(&self.code_id).unwrap_or_default());
        raw.extend_from_slice(&self.secret);
        raw.extend_from_slice(&self.panel_key);
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(raw))
    }

    /// Reads a token from text.
    pub fn parse(text: &str) -> Option<Self> {
        let raw = URL_SAFE_NO_PAD
            .decode(text.trim().strip_prefix(PREFIX)?)
            .ok()?;
        if raw.len() != 58 {
            return None;
        }
        Some(Self {
            code_id: hex::encode(&raw[..10]),
            secret: raw[10..26].try_into().ok()?,
            panel_key: raw[26..].try_into().ok()?,
        })
    }

    /// The secret as sent in the join proof.
    pub fn secret_hex(&self) -> String {
        hex::encode(self.secret)
    }

    /// What the panel stores: the SHA-256 of the secret, hex encoded.
    pub fn secret_hash(&self) -> String {
        hash_secret(&self.secret_hex())
    }
}

/// The stored form of a secret sent as hex in a join proof.
pub fn hash_secret(secret_hex: &str) -> String {
    hex::encode(Sha256::digest(secret_hex.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_junk() {
        let token = JoinToken::generate([7; 32]);
        let text = token.encode();
        assert!(text.starts_with("nnj1."));
        assert_eq!(JoinToken::parse(&text), Some(token.clone()));
        assert_eq!(
            JoinToken::parse(&format!("  {text}\n")),
            Some(token.clone())
        );
        assert_eq!(JoinToken::parse("nnj1.short"), None);
        assert_eq!(JoinToken::parse(&text.replace("nnj1.", "nnj2.")), None);
        assert_eq!(token.secret_hash(), hash_secret(&token.secret_hex()));
        assert!(!format!("{token:?}").contains(&token.secret_hex()));
    }
}
