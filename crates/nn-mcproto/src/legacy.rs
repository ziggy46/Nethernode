//! Legacy (pre-1.7) server list ping.
//!
//! Old clients open with `0xFE` instead of a length-prefixed handshake:
//! - Beta 1.8–1.3: `FE`
//! - 1.4–1.5: `FE 01`
//! - 1.6: `FE 01 FA` + an `MC|PingHost` plugin message carrying the hostname
//!
//! A modern frame whose length is exactly 254 also starts `FE 01`, but its
//! third byte is a packet ID (`00`), never `FA`, so the two are separable.
//! Source: <https://minecraft.wiki/w/Java_Edition_protocol/Server_List_Ping#Legacy_ping>

use crate::codec::Reader;
use crate::error::{DecodeError, Result};

/// Which legacy ping a client sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyPing {
    /// Beta 1.8 – 1.3 (`FE`).
    Beta,
    /// 1.4 – 1.5 (`FE 01`).
    V1_4,
    /// 1.6 (`FE 01 FA` + `MC|PingHost`). Fields are absent if the plugin
    /// message was truncated.
    V1_6 {
        /// Client protocol version.
        protocol: Option<u8>,
        /// Hostname the client connected to.
        hostname: Option<String>,
        /// Port the client connected to.
        port: Option<u32>,
    },
}

/// Result of inspecting the first bytes of a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyDetection {
    /// A modern handshake follows.
    Modern,
    /// Can't tell yet; read more (or call again with `complete = true`).
    NeedMore,
    /// A legacy ping.
    Legacy(LegacyPing),
}

/// Inspects the start of a connection. `complete` means no more bytes will
/// arrive soon (the client is waiting for a reply), which is how the bare
/// `FE` and `FE 01` forms are recognized.
pub fn detect(buf: &[u8], complete: bool) -> LegacyDetection {
    match buf {
        [] => LegacyDetection::NeedMore,
        [first, ..] if *first != 0xfe => LegacyDetection::Modern,
        [_] if complete => LegacyDetection::Legacy(LegacyPing::Beta),
        [_] => LegacyDetection::NeedMore,
        [_, second, ..] if *second != 0x01 => LegacyDetection::Modern,
        [_, _] if complete => LegacyDetection::Legacy(LegacyPing::V1_4),
        [_, _] => LegacyDetection::NeedMore,
        [_, _, third, ..] if *third != 0xfa => LegacyDetection::Modern,
        _ => match parse_ping_host(&buf[3..]) {
            Ok(ping) => LegacyDetection::Legacy(ping),
            Err(DecodeError::Incomplete) if !complete => LegacyDetection::NeedMore,
            Err(_) => LegacyDetection::Legacy(LegacyPing::V1_6 {
                protocol: None,
                hostname: None,
                port: None,
            }),
        },
    }
}

/// Upper bound on bytes a 1.6 ping can need, to cap buffering.
pub const MAX_LEGACY_PING_BYTES: usize = 3 + 2 + 22 + 2 + 1 + 2 + 255 * 2 + 4;

fn parse_ping_host(buf: &[u8]) -> Result<LegacyPing> {
    let mut r = Reader::new(buf);
    let channel_len = usize::from(r.u16()?);
    let channel = utf16be(r.bytes(channel_len * 2)?)?;
    if channel != "MC|PingHost" {
        return Err(DecodeError::InvalidValue("legacy ping channel"));
    }
    let _rest_len = r.u16()?;
    let protocol = r.u8()?;
    let host_len = usize::from(r.u16()?);
    if host_len > 255 {
        return Err(DecodeError::StringTooLong { max: 255 });
    }
    let hostname = utf16be(r.bytes(host_len * 2)?)?;
    let port_bytes = r.bytes(4)?;
    let port = u32::from_be_bytes([port_bytes[0], port_bytes[1], port_bytes[2], port_bytes[3]]);
    Ok(LegacyPing::V1_6 {
        protocol: Some(protocol),
        hostname: Some(hostname),
        port: Some(port),
    })
}

fn utf16be(bytes: &[u8]) -> Result<String> {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| DecodeError::InvalidUtf8)
}

/// Builds the `0xFF` kick packet that answers a legacy ping.
///
/// Beta clients get `motd§online§max`; 1.4+ clients get the null-delimited
/// form, reporting protocol 127 so they show the server as incompatible
/// (as 1.7+ servers do).
pub fn kick_response(
    ping: &LegacyPing,
    version_name: &str,
    motd: &str,
    online: i64,
    max: i64,
) -> Vec<u8> {
    let text = match ping {
        LegacyPing::Beta => format!("{}§{online}§{max}", motd.replace('§', "")),
        LegacyPing::V1_4 | LegacyPing::V1_6 { .. } => [
            "§1",
            "127",
            version_name,
            motd,
            &online.to_string(),
            &max.to_string(),
        ]
        .join("\0"),
    };
    let units: Vec<u16> = text.encode_utf16().take(u16::MAX as usize).collect();
    let mut out = Vec::with_capacity(3 + units.len() * 2);
    out.push(0xff);
    out.extend_from_slice(&(units.len() as u16).to_be_bytes());
    for unit in units {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 1.6 ping from the protocol documentation's example (localhost:25565).
    fn ping_1_6(host: &str) -> Vec<u8> {
        let mut out = vec![0xfe, 0x01, 0xfa, 0x00, 0x0b];
        for unit in "MC|PingHost".encode_utf16() {
            out.extend_from_slice(&unit.to_be_bytes());
        }
        let host_units: Vec<u16> = host.encode_utf16().collect();
        out.extend_from_slice(&((7 + host_units.len() * 2) as u16).to_be_bytes());
        out.push(0x4a);
        out.extend_from_slice(&(host_units.len() as u16).to_be_bytes());
        for unit in host_units {
            out.extend_from_slice(&unit.to_be_bytes());
        }
        out.extend_from_slice(&25565u32.to_be_bytes());
        out
    }

    #[test]
    fn detects_each_legacy_form() {
        assert_eq!(
            detect(&[0xfe], true),
            LegacyDetection::Legacy(LegacyPing::Beta)
        );
        assert_eq!(detect(&[0xfe], false), LegacyDetection::NeedMore);
        assert_eq!(
            detect(&[0xfe, 0x01], true),
            LegacyDetection::Legacy(LegacyPing::V1_4)
        );
        assert_eq!(
            detect(&ping_1_6("Play.Example.com"), false),
            LegacyDetection::Legacy(LegacyPing::V1_6 {
                protocol: Some(74),
                hostname: Some("Play.Example.com".into()),
                port: Some(25565),
            })
        );
        // Truncated 1.6 ping: wait, then give up on the details.
        let partial = &ping_1_6("localhost")[..10];
        assert_eq!(detect(partial, false), LegacyDetection::NeedMore);
        assert!(matches!(
            detect(partial, true),
            LegacyDetection::Legacy(LegacyPing::V1_6 { hostname: None, .. })
        ));
    }

    #[test]
    fn modern_frames_are_not_mistaken_for_legacy() {
        // Ordinary handshake.
        assert_eq!(detect(&[0x10, 0x00], false), LegacyDetection::Modern);
        // A modern frame of exactly 254 bytes: FE 01, then packet ID 00.
        assert_eq!(detect(&[0xfe, 0x01, 0x00], false), LegacyDetection::Modern);
        // A modern frame of 382 bytes: FE 02.
        assert_eq!(detect(&[0xfe, 0x02], false), LegacyDetection::Modern);
    }

    #[test]
    fn builds_kick_responses() {
        // The documented example: ff 00 17 + "A Minecraft Server§0§10".
        let beta = kick_response(&LegacyPing::Beta, "1.21", "A Minecraft Server", 0, 10);
        assert_eq!(&beta[..3], &[0xff, 0x00, 0x17]);
        assert_eq!(utf16be(&beta[3..]).unwrap(), "A Minecraft Server§0§10");
        // § would corrupt the Beta field separators, so it's stripped.
        let beta = kick_response(&LegacyPing::Beta, "1.21", "§6Gold", 0, 10);
        assert_eq!(utf16be(&beta[3..]).unwrap(), "6Gold§0§10");

        let modern = kick_response(&LegacyPing::V1_4, "NetherNode", "Hello", 1, 20);
        assert_eq!(
            utf16be(&modern[3..]).unwrap(),
            ["§1", "127", "NetherNode", "Hello", "1", "20"].join("\0")
        );
    }

    #[test]
    fn detect_never_panics_on_garbage() {
        for len in 0..80 {
            let buf: Vec<u8> = (0..len)
                .map(|i| {
                    if i < 3 {
                        [0xfe, 0x01, 0xfa][i]
                    } else {
                        (i * 37) as u8
                    }
                })
                .collect();
            let _ = detect(&buf, true);
            let _ = detect(&buf, false);
        }
    }
}
