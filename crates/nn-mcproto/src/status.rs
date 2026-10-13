//! Status (server list ping) packets and the response JSON.
//! Source: <https://minecraft.wiki/w/Java_Edition_protocol/Server_List_Ping>

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::codec::{Reader, write_string};
use crate::error::{DecodeError, Result};
use crate::varint::write_varint;

/// Status Request (serverbound) and Status Response (clientbound) ID.
pub const STATUS_ID: i32 = 0x00;
/// Ping Request (serverbound) and Pong Response (clientbound) ID.
pub const PING_ID: i32 = 0x01;
/// Maximum Status Response JSON length, in UTF-16 code units.
pub const MAX_STATUS_JSON_CHARS: usize = 32_767;

/// The Status Response JSON. Unknown fields (e.g. Forge's `forgeData`) are
/// kept so cached responses replay faithfully.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusResponse {
    /// Server version name and protocol number.
    pub version: StatusVersion,
    /// Player counts. Clients show "???" when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players: Option<StatusPlayers>,
    /// MOTD as a text component (string or object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Value>,
    /// `data:image/png;base64,…` 64×64 icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
    /// Any other fields, passed through untouched.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `version` in the status JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusVersion {
    /// Display name, e.g. "Paper 1.21.4".
    pub name: String,
    /// Protocol number; clients flag a mismatch as incompatible.
    pub protocol: i32,
}

/// `players` in the status JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusPlayers {
    /// Maximum players.
    pub max: i64,
    /// Players online.
    pub online: i64,
    /// Sample of online players for the hover tooltip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<Vec<Value>>,
}

impl StatusResponse {
    /// A minimal response with a plain-text MOTD.
    pub fn simple(version_name: &str, protocol: i32, motd: &str, online: i64, max: i64) -> Self {
        Self {
            version: StatusVersion {
                name: version_name.into(),
                protocol,
            },
            players: Some(StatusPlayers {
                max,
                online,
                sample: None,
            }),
            description: Some(json!({ "text": motd })),
            favicon: None,
            extra: Map::new(),
        }
    }

    /// Parses a Status Response JSON string.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|_| DecodeError::InvalidValue("status JSON"))
    }

    /// Serializes to the JSON sent on the wire.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// Appends `suffix` (plain text, gray) to the MOTD.
    pub fn append_to_description(&mut self, suffix: &str) {
        let original = self.description.take().unwrap_or_else(|| json!(""));
        self.description = Some(json!({
            "text": "",
            "extra": [original, { "text": suffix, "color": "gray" }],
        }));
    }

    /// Sets the online count to zero and clears the player sample.
    pub fn clear_players(&mut self) {
        if let Some(players) = &mut self.players {
            players.online = 0;
            players.sample = None;
        }
    }
}

/// Encodes a Status Response packet payload carrying `json`.
pub fn encode_status_response(json: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(json.len() + 4);
    write_varint(&mut out, STATUS_ID);
    write_string(&mut out, json);
    out
}

/// Decodes a Status Response payload, returning the raw JSON string.
pub fn decode_status_response(payload: &[u8]) -> Result<String> {
    let mut r = Reader::new(payload);
    let id = r.varint()?;
    if id != STATUS_ID {
        return Err(DecodeError::UnexpectedPacketId(id));
    }
    Ok(r.string(MAX_STATUS_JSON_CHARS)?.to_owned())
}

/// Encodes a Status Request payload (no fields).
pub fn encode_status_request() -> Vec<u8> {
    vec![STATUS_ID as u8]
}

/// Checks that a payload is a Status Request.
pub fn decode_status_request(payload: &[u8]) -> Result<()> {
    match Reader::new(payload).varint()? {
        STATUS_ID => Ok(()),
        other => Err(DecodeError::UnexpectedPacketId(other)),
    }
}

/// Encodes a Ping Request or Pong Response payload with `timestamp`.
pub fn encode_ping(timestamp: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    write_varint(&mut out, PING_ID);
    out.extend_from_slice(&timestamp.to_be_bytes());
    out
}

/// Decodes a Ping Request (or Pong Response) payload, returning its timestamp.
pub fn decode_ping(payload: &[u8]) -> Result<i64> {
    let mut r = Reader::new(payload);
    match r.varint()? {
        PING_ID => r.i64(),
        other => Err(DecodeError::UnexpectedPacketId(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_round_trips_and_keeps_unknown_fields() {
        let raw = r#"{"version":{"name":"Forge 1.20.1","protocol":763},"players":{"max":20,"online":3,"sample":[{"name":"Alex","id":"ec561538-f3fd-461d-aff5-086b22154bce"}]},"description":"A §6Forge§r server","favicon":"data:image/png;base64,AAAA","forgeData":{"fmlNetworkVersion":3},"enforcesSecureChat":true}"#;
        let status = StatusResponse::from_json(raw).unwrap();
        assert_eq!(status.version.protocol, 763);
        assert!(status.extra.contains_key("forgeData"));
        assert!(status.extra.contains_key("enforcesSecureChat"));
        let again = StatusResponse::from_json(&status.to_json()).unwrap();
        assert_eq!(again, status);
    }

    #[test]
    fn sleeping_transform() {
        let mut status = StatusResponse::from_json(
            r#"{"version":{"name":"Paper","protocol":777},"players":{"max":20,"online":5,"sample":[]},"description":{"text":"Hi"}}"#,
        )
        .unwrap();
        status.clear_players();
        status.append_to_description(" 💤 Sleeping — join to wake");
        let players = status.players.as_ref().unwrap();
        assert_eq!((players.online, players.max), (0, 20));
        assert!(players.sample.is_none());
        let desc = status.description.unwrap();
        assert_eq!(desc["extra"][0]["text"], "Hi");
        assert_eq!(desc["extra"][1]["text"], " 💤 Sleeping — join to wake");
    }

    #[test]
    fn packets_round_trip() {
        let json = StatusResponse::simple("NetherNode", 777, "Unknown server", 0, 0).to_json();
        assert_eq!(
            decode_status_response(&encode_status_response(&json)).unwrap(),
            json
        );
        assert_eq!(decode_ping(&encode_ping(123_456)).unwrap(), 123_456);
        assert!(decode_status_request(&encode_status_request()).is_ok());
        assert_eq!(
            decode_status_request(&[0x01]),
            Err(DecodeError::UnexpectedPacketId(1))
        );
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(StatusResponse::from_json("{}").is_err());
        assert!(StatusResponse::from_json("not json").is_err());
    }
}
