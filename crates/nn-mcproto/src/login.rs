//! Login-state packets the proxy needs: Login Start (to learn who is joining)
//! and Disconnect (to turn a player away with a message).
//!
//! Login Start's layout changed several times. Field order by protocol:
//! - < 759 (before 1.19): name
//! - 759 (1.19): name, has-signature bool, [expiry i64, key bytes, signature bytes]
//! - 760 (1.19.1–1.19.2): as 759, then has-UUID bool, [UUID]
//! - 761–763 (1.19.3–1.20.1): name, has-UUID bool, [UUID]
//! - ≥ 764 (1.20.2+): name, UUID
//!
//! Sources: <https://minecraft.wiki/w/Java_Edition_protocol/Packets#Login_Start>,
//! Velocity's `ServerLoginPacket` decoder
//! (<https://github.com/PaperMC/Velocity/blob/dev/3.0.0/proxy/src/main/java/com/velocitypowered/proxy/protocol/packet/ServerLoginPacket.java>).

use serde_json::json;
use uuid::Uuid;

use crate::codec::{Reader, write_string};
use crate::error::{DecodeError, Result};
use crate::varint::write_varint;

/// Login Start (serverbound) packet ID.
pub const LOGIN_START_ID: i32 = 0x00;
/// Disconnect (clientbound, login state) packet ID.
pub const LOGIN_DISCONNECT_ID: i32 = 0x00;
/// Maximum username length.
pub const MAX_USERNAME_CHARS: usize = 16;

/// 1.19
pub const PROTOCOL_1_19: i32 = 759;
/// 1.19.1 and 1.19.2
pub const PROTOCOL_1_19_1: i32 = 760;
/// 1.19.3
pub const PROTOCOL_1_19_3: i32 = 761;
/// 1.20.2
pub const PROTOCOL_1_20_2: i32 = 764;

/// A decoded Login Start packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginStart {
    /// Username the client claims (unverified until the backend authenticates).
    pub username: String,
    /// The client's UUID, when its protocol version sends one.
    pub uuid: Option<Uuid>,
}

impl LoginStart {
    /// Decodes Login Start for a client speaking `protocol`.
    pub fn decode(payload: &[u8], protocol: i32) -> Result<Self> {
        let mut r = Reader::new(payload);
        let id = r.varint()?;
        if id != LOGIN_START_ID {
            return Err(DecodeError::UnexpectedPacketId(id));
        }
        let username = r.string(MAX_USERNAME_CHARS)?.to_owned();
        if username.is_empty() {
            return Err(DecodeError::InvalidValue("username"));
        }

        if (PROTOCOL_1_19..PROTOCOL_1_19_3).contains(&protocol) && r.bool()? {
            // Signature data (removed in 1.19.3): expiry, public key, signature.
            r.i64()?;
            r.byte_array(512)?;
            r.byte_array(4096)?;
        }

        // 1.20.2+ always sends the UUID; 1.19.1–1.20.1 prefix it with a flag.
        let has_uuid = protocol >= PROTOCOL_1_20_2 || (protocol >= PROTOCOL_1_19_1 && r.bool()?);
        let uuid = if has_uuid { Some(r.uuid()?) } else { None };
        Ok(Self { username, uuid })
    }

    /// Encodes Login Start for `protocol`. Used by tests and load tools.
    pub fn encode(&self, protocol: i32) -> Vec<u8> {
        let mut out = Vec::new();
        write_varint(&mut out, LOGIN_START_ID);
        write_string(&mut out, &self.username);
        if (PROTOCOL_1_19..PROTOCOL_1_19_3).contains(&protocol) {
            out.push(0); // no signature data
        }
        if protocol >= PROTOCOL_1_20_2 {
            out.extend_from_slice(self.uuid.unwrap_or_default().as_bytes());
        } else if protocol >= PROTOCOL_1_19_1 {
            match self.uuid {
                Some(uuid) => {
                    out.push(1);
                    out.extend_from_slice(uuid.as_bytes());
                }
                None => out.push(0),
            }
        }
        out
    }
}

/// Encodes a login-state Disconnect packet payload with a plain-text reason.
///
/// The reason is a JSON text component in the login state for every version
/// since 1.7. Source: <https://minecraft.wiki/w/Java_Edition_protocol/Packets#Disconnect_(login)>
pub fn login_disconnect(reason: &str) -> Vec<u8> {
    let mut out = Vec::new();
    write_varint(&mut out, LOGIN_DISCONNECT_ID);
    write_string(&mut out, &json!({ "text": reason }).to_string());
    out
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::codec::Reader;

    const UUID: Uuid = Uuid::from_u128(0x069a79f4_44e9_4726_a5be_fca90e38aaf5);

    #[test]
    fn decodes_every_layout() {
        let named = |uuid| LoginStart {
            username: "Notch".into(),
            uuid,
        };
        for (protocol, uuid) in [
            (758, None),       // 1.18.2: name only
            (759, None),       // 1.19: signature data, no UUID
            (760, Some(UUID)), // 1.19.2: optional UUID present
            (760, None),       // 1.19.2: optional UUID absent
            (763, Some(UUID)), // 1.20.1
            (764, Some(UUID)), // 1.20.2: always present
            (777, Some(UUID)), // current
        ] {
            let packet = named(uuid);
            assert_eq!(
                LoginStart::decode(&packet.encode(protocol), protocol).unwrap(),
                packet,
                "{protocol}"
            );
        }
    }

    #[test]
    fn skips_1_19_signature_data() {
        let mut payload = vec![0x00];
        write_string(&mut payload, "Steve");
        payload.push(1); // has signature data
        payload.extend_from_slice(&1_700_000_000_000i64.to_be_bytes());
        payload.extend_from_slice(&[0x03, 1, 2, 3]); // key
        payload.extend_from_slice(&[0x02, 9, 9]); // signature
        payload.push(1); // has UUID (1.19.1+)
        payload.extend_from_slice(UUID.as_bytes());
        let decoded = LoginStart::decode(&payload, 760).unwrap();
        assert_eq!(decoded.username, "Steve");
        assert_eq!(decoded.uuid, Some(UUID));
    }

    #[test]
    fn rejects_bad_login_start() {
        let mut long = vec![0x00];
        write_string(&mut long, &"x".repeat(17));
        assert!(LoginStart::decode(&long, 777).is_err());
        let mut empty = vec![0x00];
        write_string(&mut empty, "");
        assert!(LoginStart::decode(&empty, 777).is_err());
        assert_eq!(
            LoginStart::decode(&[0x01], 777),
            Err(DecodeError::UnexpectedPacketId(1))
        );
    }

    #[test]
    fn disconnect_carries_a_json_text_component() {
        let payload = login_disconnect("Server is starting");
        let mut r = Reader::new(&payload);
        assert_eq!(r.varint().unwrap(), LOGIN_DISCONNECT_ID);
        let json: serde_json::Value = serde_json::from_str(r.string(262_144).unwrap()).unwrap();
        assert_eq!(json["text"], "Server is starting");
    }

    proptest! {
        #[test]
        fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200), protocol in 0i32..900) {
            let _ = LoginStart::decode(&bytes, protocol);
        }
    }
}
