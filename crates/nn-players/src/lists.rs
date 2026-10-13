//! Minecraft's player lists on disk: `ops.json`, `whitelist.json`,
//! `banned-players.json`, `banned-ips.json`, and `usercache.json`.
//!
//! A running server keeps these in memory and rewrites them on change, so
//! they're read freely but edited only while the server is stopped (it
//! loads them on start). Formats and the `yyyy-MM-dd HH:mm:ss +0000` date
//! style are as written by vanilla 26.3.

use serde::{Deserialize, Serialize};

use crate::profile::{Profile, normalize_uuid};

/// File names, relative to the server folder.
pub const OPS: &str = "ops.json";
/// See [`OPS`].
pub const WHITELIST: &str = "whitelist.json";
/// See [`OPS`].
pub const BANNED_PLAYERS: &str = "banned-players.json";
/// See [`OPS`].
pub const BANNED_IPS: &str = "banned-ips.json";
/// See [`OPS`].
pub const USER_CACHE: &str = "usercache.json";

/// `expires` value for permanent bans.
pub const FOREVER: &str = "forever";

/// An `ops.json` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpEntry {
    /// Hyphenated UUID.
    pub uuid: String,
    /// Username.
    pub name: String,
    /// 1–4.
    pub level: u8,
    /// May join when full.
    #[serde(default)]
    pub bypasses_player_limit: bool,
}

/// A `whitelist.json` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    /// Hyphenated UUID.
    pub uuid: String,
    /// Username.
    pub name: String,
}

/// A `banned-players.json` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BanEntry {
    /// Hyphenated UUID.
    pub uuid: String,
    /// Username.
    pub name: String,
    /// When, in Minecraft's date format.
    pub created: String,
    /// Who.
    pub source: String,
    /// Minecraft's date format, or [`FOREVER`].
    pub expires: String,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A `banned-ips.json` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpBanEntry {
    /// The address.
    pub ip: String,
    /// When, in Minecraft's date format.
    pub created: String,
    /// Who.
    pub source: String,
    /// Minecraft's date format, or [`FOREVER`].
    pub expires: String,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A `usercache.json` entry: players the server has looked up or seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    /// Hyphenated UUID.
    pub uuid: String,
    /// Username.
    pub name: String,
    /// Minecraft's date format.
    #[serde(default)]
    pub expires_on: String,
}

/// Parses a list file. A missing or empty file is an empty list; entries
/// that don't parse are skipped rather than failing the whole list.
pub fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Vec<T> {
    let Ok(values) = serde_json::from_slice::<Vec<serde_json::Value>>(bytes) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

/// Serializes a list the way Minecraft does (two-space indented JSON).
pub fn to_bytes<T: Serialize>(list: &[T]) -> Vec<u8> {
    serde_json::to_vec_pretty(list).unwrap_or_else(|_| b"[]".to_vec())
}

/// A unix-ms time in Minecraft's list date format.
pub fn mc_date(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(jiff::tz::TimeZone::UTC)
        .strftime("%Y-%m-%d %H:%M:%S %z")
        .to_string()
}

/// Parses Minecraft's list date format. `None` for [`FOREVER`] or garbage.
pub fn parse_mc_date(text: &str) -> Option<i64> {
    if text == FOREVER {
        return None;
    }
    jiff::fmt::strtime::parse("%Y-%m-%d %H:%M:%S %z", text)
        .ok()?
        .to_timestamp()
        .ok()
        .map(|t| t.as_millisecond())
}

/// Whether two entries refer to the same player: same UUID, or (for
/// entries without a valid UUID) the same name, ignoring case.
fn same(uuid: &str, name: &str, target: &Profile) -> bool {
    match normalize_uuid(uuid) {
        Some(u) => u == target.uuid,
        None => name.eq_ignore_ascii_case(&target.name),
    }
}

/// Adds or replaces `player` in an ops list. Returns whether it changed.
pub fn set_op(list: &mut Vec<OpEntry>, player: &Profile, level: u8) -> bool {
    if let Some(entry) = list.iter_mut().find(|e| same(&e.uuid, &e.name, player)) {
        let changed = entry.level != level || entry.name != player.name;
        entry.level = level;
        entry.name.clone_from(&player.name);
        return changed;
    }
    list.push(OpEntry {
        uuid: player.uuid.clone(),
        name: player.name.clone(),
        level,
        bypasses_player_limit: false,
    });
    true
}

/// Removes `player` from an ops list. Returns whether it was there.
pub fn remove_op(list: &mut Vec<OpEntry>, player: &Profile) -> bool {
    let before = list.len();
    list.retain(|e| !same(&e.uuid, &e.name, player));
    list.len() != before
}

/// Adds `player` to the whitelist. Returns whether it changed.
pub fn add_listed(list: &mut Vec<ListEntry>, player: &Profile) -> bool {
    if list.iter().any(|e| same(&e.uuid, &e.name, player)) {
        return false;
    }
    list.push(ListEntry {
        uuid: player.uuid.clone(),
        name: player.name.clone(),
    });
    true
}

/// Removes `player` from the whitelist. Returns whether it was there.
pub fn remove_listed(list: &mut Vec<ListEntry>, player: &Profile) -> bool {
    let before = list.len();
    list.retain(|e| !same(&e.uuid, &e.name, player));
    list.len() != before
}

/// Bans `player`, replacing an existing ban. `expires_ms` `None` is permanent.
pub fn ban(
    list: &mut Vec<BanEntry>,
    player: &Profile,
    reason: Option<&str>,
    source: &str,
    now_ms: i64,
    expires_ms: Option<i64>,
) {
    list.retain(|e| !same(&e.uuid, &e.name, player));
    list.push(BanEntry {
        uuid: player.uuid.clone(),
        name: player.name.clone(),
        created: mc_date(now_ms),
        source: source.to_owned(),
        expires: expires_ms.map_or_else(|| FOREVER.to_owned(), mc_date),
        reason: reason.map(str::to_owned),
    });
}

/// Lifts a ban. Returns whether there was one.
pub fn pardon(list: &mut Vec<BanEntry>, player: &Profile) -> bool {
    let before = list.len();
    list.retain(|e| !same(&e.uuid, &e.name, player));
    list.len() != before
}

/// Bans an IP, replacing an existing ban.
pub fn ban_ip(
    list: &mut Vec<IpBanEntry>,
    ip: &str,
    reason: Option<&str>,
    source: &str,
    now_ms: i64,
    expires_ms: Option<i64>,
) {
    list.retain(|e| e.ip != ip);
    list.push(IpBanEntry {
        ip: ip.to_owned(),
        created: mc_date(now_ms),
        source: source.to_owned(),
        expires: expires_ms.map_or_else(|| FOREVER.to_owned(), mc_date),
        reason: reason.map(str::to_owned),
    });
}

/// Lifts an IP ban. Returns whether there was one.
pub fn pardon_ip(list: &mut Vec<IpBanEntry>, ip: &str) -> bool {
    let before = list.len();
    list.retain(|e| e.ip != ip);
    list.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANS: &str = r#"[
  {
    "uuid": "6b4d15f3-c715-3d2f-9d82-512bca981c70",
    "name": "xz9qq_unknown_player",
    "created": "2026-10-13 22:11:14 +0000",
    "source": "Rcon",
    "expires": "forever"
  },
  {
    "uuid": "069a79f4-44e9-4726-a5be-fca90e38aaf5",
    "name": "Notch",
    "created": "2026-10-13 22:11:15 +0000",
    "source": "Management server",
    "expires": "2030-01-02 03:04:05 +0000",
    "reason": "timed"
  }
]"#;

    fn notch() -> Profile {
        Profile {
            uuid: "069a79f4-44e9-4726-a5be-fca90e38aaf5".into(),
            name: "Notch".into(),
        }
    }

    #[test]
    fn reads_what_the_server_writes() {
        let bans: Vec<BanEntry> = parse(BANS.as_bytes());
        assert_eq!(bans.len(), 2);
        assert_eq!(bans[0].expires, FOREVER);
        assert_eq!(bans[1].reason.as_deref(), Some("timed"));
        assert_eq!(parse_mc_date(&bans[1].expires), Some(1_893_553_445_000));
        assert_eq!(parse_mc_date(FOREVER), None);
        assert_eq!(mc_date(1_893_553_445_000), "2030-01-02 03:04:05 +0000");
        assert!(parse::<BanEntry>(b"").is_empty());
        assert!(parse::<BanEntry>(b"not json").is_empty());
        let mixed: Vec<ListEntry> = parse(br#"[{"uuid":"x","name":"a"},{"bogus":1}]"#);
        assert_eq!(mixed.len(), 1, "bad entries are skipped");
    }

    #[test]
    fn edits() {
        let mut bans: Vec<BanEntry> = parse(BANS.as_bytes());
        ban(&mut bans, &notch(), Some("again"), "NetherNode", 0, None);
        assert_eq!(bans.len(), 2, "replaced, not duplicated");
        assert_eq!(bans[1].expires, FOREVER);
        assert!(pardon(&mut bans, &notch()));
        assert!(!pardon(&mut bans, &notch()));

        let mut ops = Vec::new();
        assert!(set_op(&mut ops, &notch(), 4));
        assert!(!set_op(&mut ops, &notch(), 4));
        assert!(set_op(&mut ops, &notch(), 2));
        assert_eq!(ops.len(), 1);
        assert!(remove_op(&mut ops, &notch()));

        let mut wl = Vec::new();
        assert!(add_listed(&mut wl, &notch()));
        assert!(!add_listed(&mut wl, &notch()));
        let text = String::from_utf8(to_bytes(&wl)).unwrap();
        assert!(text.contains("\n  {"), "pretty-printed like Minecraft");
        assert!(remove_listed(&mut wl, &notch()));

        let mut ips = Vec::new();
        ban_ip(
            &mut ips,
            "203.0.113.7",
            None,
            "NetherNode",
            0,
            Some(1_893_553_445_000),
        );
        assert_eq!(ips[0].expires, "2030-01-02 03:04:05 +0000");
        assert!(pardon_ip(&mut ips, "203.0.113.7"));
    }
}
