//! The Handshake packet: the first packet of every modern connection.
//! Source: <https://minecraft.wiki/w/Java_Edition_protocol/Packets#Handshake>

use crate::codec::{Reader, write_string};
use crate::error::{DecodeError, Result};
use crate::varint::write_varint;

/// Handshake packet ID.
pub const HANDSHAKE_ID: i32 = 0x00;
/// Maximum server address length, in UTF-16 code units.
pub const MAX_ADDRESS_CHARS: usize = 255;

/// What the client wants after the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NextState {
    /// Server list ping.
    Status,
    /// Joining the game.
    Login,
    /// Joining after a server transfer (1.20.5+). Proceeds like login.
    Transfer,
}

impl NextState {
    /// Whether this state proceeds to login.
    pub fn is_login(self) -> bool {
        matches!(self, NextState::Login | NextState::Transfer)
    }

    fn from_i32(v: i32) -> Result<Self> {
        match v {
            1 => Ok(NextState::Status),
            2 => Ok(NextState::Login),
            3 => Ok(NextState::Transfer),
            other => Err(DecodeError::InvalidNextState(other)),
        }
    }

    fn as_i32(self) -> i32 {
        match self {
            NextState::Status => 1,
            NextState::Login => 2,
            NextState::Transfer => 3,
        }
    }
}

/// A decoded Handshake packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// Client protocol version.
    pub protocol_version: i32,
    /// The address exactly as sent, including any Forge or forwarding suffix.
    pub server_address: String,
    /// The port the client connected to (informational).
    pub server_port: u16,
    /// Requested next state.
    pub next_state: NextState,
}

impl Handshake {
    /// Decodes a Handshake from a frame payload (packet ID + fields).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = Reader::new(payload);
        let id = r.varint()?;
        if id != HANDSHAKE_ID {
            return Err(DecodeError::UnexpectedPacketId(id));
        }
        let protocol_version = r.varint()?;
        let server_address = r.string(MAX_ADDRESS_CHARS)?.to_owned();
        let server_port = r.u16()?;
        let next_state = NextState::from_i32(r.varint()?)?;
        Ok(Self {
            protocol_version,
            server_address,
            server_port,
            next_state,
        })
    }

    /// Encodes the packet payload (without the frame length).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.server_address.len() + 10);
        write_varint(&mut out, HANDSHAKE_ID);
        write_varint(&mut out, self.protocol_version);
        write_string(&mut out, &self.server_address);
        out.extend_from_slice(&self.server_port.to_be_bytes());
        write_varint(&mut out, self.next_state.as_i32());
        out
    }

    /// The hostname to route on. See [`normalize_hostname`].
    pub fn hostname(&self) -> String {
        normalize_hostname(&self.server_address)
    }

    /// The Forge/FML marker (`FML`, `FML2`, `FML3`) after the address, if any.
    pub fn forge_marker(&self) -> Option<&str> {
        self.server_address
            .split('\0')
            .skip(1)
            .find(|part| part.starts_with("FML"))
    }
}

/// Normalizes a handshake address for routing:
///
/// 1. drop everything from the first `\0` (Forge `\0FML\0`, `\0FML2\0`,
///    `\0FML3\0` markers and BungeeCord-style forwarding data)
/// 2. drop a port suffix (`host:25565`, `[::1]:25565`)
/// 3. drop trailing dots (some clients send the FQDN form `host.`)
/// 4. lowercase
///
/// Anything that isn't then a plausible hostname (letters, digits, `-`,
/// `_`, `.`) or IPv6 literal becomes the empty string, which matches no
/// hostname route. Normalizing a normalized value returns it unchanged.
pub fn normalize_hostname(address: &str) -> String {
    let host = address.split('\0').next().unwrap_or_default().trim();
    let host = if let Some(rest) = host.strip_prefix('[') {
        // Bracketed IPv6, possibly with a port.
        rest.split(']').next().unwrap_or_default()
    } else if host.matches(':').count() == 1 {
        host.split(':').next().unwrap_or_default()
    } else {
        // No port, or a bare IPv6 literal (which can't carry one).
        host
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if is_plausible_host(&host) {
        host
    } else {
        String::new()
    }
}

fn is_plausible_host(host: &str) -> bool {
    if host.contains(':') {
        return host.parse::<std::net::Ipv6Addr>().is_ok();
    }
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn handshake(address: &str) -> Handshake {
        Handshake {
            protocol_version: 777,
            server_address: address.into(),
            server_port: 25565,
            next_state: NextState::Login,
        }
    }

    #[test]
    fn decodes_a_real_handshake() {
        // 1.21.4 client (protocol 769) pinging "localhost":25565.
        let payload = [
            0x00, 0x81, 0x06, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0x63,
            0xdd, 0x01,
        ];
        let hs = Handshake::decode(&payload).unwrap();
        assert_eq!(hs.protocol_version, 769);
        assert_eq!(hs.server_address, "localhost");
        assert_eq!(hs.server_port, 25565);
        assert_eq!(hs.next_state, NextState::Status);
    }

    #[test]
    fn round_trips() {
        for state in [NextState::Status, NextState::Login, NextState::Transfer] {
            let hs = Handshake {
                next_state: state,
                ..handshake("play.example.com\0FML3\0")
            };
            assert_eq!(Handshake::decode(&hs.encode()).unwrap(), hs);
        }
    }

    #[test]
    fn rejects_bad_handshakes() {
        let mut bad_state = handshake("a").encode();
        *bad_state.last_mut().unwrap() = 4;
        assert_eq!(
            Handshake::decode(&bad_state),
            Err(DecodeError::InvalidNextState(4))
        );

        let mut bad_id = handshake("a").encode();
        bad_id[0] = 0x01;
        assert_eq!(
            Handshake::decode(&bad_id),
            Err(DecodeError::UnexpectedPacketId(1))
        );

        let long = handshake(&"a".repeat(256)).encode();
        assert!(matches!(
            Handshake::decode(&long),
            Err(DecodeError::StringTooLong { .. })
        ));
        assert!(Handshake::decode(&handshake(&"a".repeat(255)).encode()).is_ok());

        let truncated = &handshake("example.com").encode()[..8];
        assert_eq!(Handshake::decode(truncated), Err(DecodeError::Incomplete));
    }

    #[test]
    fn normalizes_hostnames() {
        let cases = [
            ("Survival.Example.COM", "survival.example.com"),
            ("survival.example.com.", "survival.example.com"),
            ("survival.example.com\0FML\0", "survival.example.com"),
            ("survival.example.com\0FML2\0", "survival.example.com"),
            ("Survival.Example.com.\0FML3\0", "survival.example.com"),
            ("survival.example.com:25565", "survival.example.com"),
            ("survival.example.com.:25565", "survival.example.com"),
            ("[::1]:25565", "::1"),
            ("::1", "::1"),
            ("10.0.0.5", "10.0.0.5"),
            ("host\u{0}203.0.113.9\u{0}uuid", "host"),
            ("", ""),
            // Found by the fuzzer: whitespace hiding before a trailing dot.
            ("a\x0c.\0\0.com", ""),
            ("bad host.com", ""),
            ("[a:b]:25565", ""),
            ("play.example.com\u{0}FML3\u{0}", "play.example.com"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_hostname(input), expected, "{input:?}");
        }
    }

    #[test]
    fn detects_forge_markers() {
        assert_eq!(handshake("a.com\0FML3\0").forge_marker(), Some("FML3"));
        assert_eq!(handshake("a.com\0FML\0").forge_marker(), Some("FML"));
        assert_eq!(handshake("a.com").forge_marker(), None);
    }

    proptest! {
        #[test]
        fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
            let _ = Handshake::decode(&bytes);
        }

        #[test]
        fn normalization_is_idempotent(address in "\\PC{0,64}") {
            let once = normalize_hostname(&address);
            prop_assert!(!once.contains('\0'));
            prop_assert!(!once.ends_with('.'));
            prop_assert_eq!(normalize_hostname(&once), once);
        }
    }
}
