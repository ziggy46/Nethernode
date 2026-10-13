//! Player identity: names, UUIDs, and how a server matches them.
//!
//! Online-mode servers identify players by their Mojang account UUID.
//! Offline-mode servers derive a UUID from the exact username
//! (`OfflinePlayer:<name>`, MD5, version 3), so a list entry only matches
//! a joining player if it carries that same UUID. Every backend passes
//! explicit UUIDs computed here rather than letting the server guess.

use md5::{Digest, Md5};

/// A player as Minecraft's lists store them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Profile {
    /// Hyphenated lowercase UUID.
    #[serde(rename = "id")]
    pub uuid: String,
    /// Username with its proper capitalization.
    pub name: String,
}

/// Whether `name` is a valid Minecraft username: 1–16 letters, digits, or
/// underscores. Anything else is refused before it reaches a command line.
pub fn valid_name(name: &str) -> bool {
    (1..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The UUID an offline-mode server gives `name`.
pub fn offline_uuid(name: &str) -> String {
    let digest: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    uuid::Builder::from_md5_bytes(digest)
        .into_uuid()
        .hyphenated()
        .to_string()
}

/// Normalizes a UUID with or without hyphens to hyphenated lowercase.
pub fn normalize_uuid(raw: &str) -> Option<String> {
    uuid::Uuid::try_parse(raw.trim())
        .ok()
        .map(|u| u.hyphenated().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["jeb_", "Notch", "a", "Sixteen_Chars_16"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "ThisNameIsWayTooLong",
            "@a",
            "a b",
            "name;op",
            "ä",
            "-x",
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn offline_uuids_match_the_server() {
        // Values a 26.3 server in offline mode produced.
        assert_eq!(
            offline_uuid("xz9qq_unknown_player"),
            "6b4d15f3-c715-3d2f-9d82-512bca981c70"
        );
        assert_eq!(
            offline_uuid("offlineguy2"),
            "eacc245c-67fd-321c-89e6-37718d2824f3"
        );
        assert_ne!(
            offline_uuid("OfflineGuy2"),
            offline_uuid("offlineguy2"),
            "case matters"
        );
    }

    #[test]
    fn uuid_forms() {
        assert_eq!(
            normalize_uuid("069A79F444E94726A5BEFCA90E38AAF5").as_deref(),
            Some("069a79f4-44e9-4726-a5be-fca90e38aaf5")
        );
        assert_eq!(normalize_uuid("not-a-uuid"), None);
    }
}
