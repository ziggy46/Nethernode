//! Minecraft Bedrock Edition's server list ping, which rides on RakNet's
//! "unconnected" packets over UDP.
//!
//! A Bedrock client fills its server list by sending an Unconnected Ping and
//! reading the Unconnected Pong, whose payload is one semicolon-separated
//! string (name, versions, players, and so on). Joining starts with an Open
//! Connection Request 1. Nothing after that is parsed here: the proxy relays
//! the rest of a session as it is.
//!
//! Reference: <https://minecraft.wiki/w/RakNet> and
//! <https://wiki.vg/Raknet_Protocol>

/// RakNet's "offline message" marker, in every unconnected packet.
pub const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

/// Unconnected Ping.
pub const ID_UNCONNECTED_PING: u8 = 0x01;
/// Unconnected Ping, answered only by servers with room (clients use
/// this when looking for LAN games).
pub const ID_UNCONNECTED_PING_OPEN: u8 = 0x02;
/// Open Connection Request 1: the first packet of a join.
pub const ID_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
/// Unconnected Pong.
pub const ID_UNCONNECTED_PONG: u8 = 0x1c;

/// The largest datagram the proxy relays (RakNet's biggest MTU is 1492).
pub const MAX_DATAGRAM: usize = 2048;

/// What a datagram from a client is, as far as routing cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packet {
    /// A server list ping, with the time the client put in it.
    Ping {
        /// The client's timestamp, echoed in the pong.
        time: i64,
    },
    /// A client starting to join.
    OpenConnection,
    /// Anything else (part of an established session, or noise).
    Other,
}

/// Classifies a datagram received on a listener with no session for its
/// sender.
pub fn classify(data: &[u8]) -> Packet {
    match data.first() {
        Some(&ID_UNCONNECTED_PING | &ID_UNCONNECTED_PING_OPEN) if data.len() >= 33 => {
            if data[9..25] != MAGIC {
                return Packet::Other;
            }
            let mut time = [0u8; 8];
            time.copy_from_slice(&data[1..9]);
            Packet::Ping {
                time: i64::from_be_bytes(time),
            }
        }
        Some(&ID_OPEN_CONNECTION_REQUEST_1) if data.len() >= 18 && data[1..17] == MAGIC => {
            Packet::OpenConnection
        }
        _ => Packet::Other,
    }
}

/// An Unconnected Ping.
pub fn encode_ping(time: i64, client_guid: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(33);
    out.push(ID_UNCONNECTED_PING);
    out.extend_from_slice(&time.to_be_bytes());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&client_guid.to_be_bytes());
    out
}

/// An Unconnected Pong carrying `status`.
pub fn encode_pong(time: i64, server_guid: i64, status: &str) -> Vec<u8> {
    let text = status.as_bytes();
    let len = u16::try_from(text.len()).unwrap_or(u16::MAX);
    let mut out = Vec::with_capacity(35 + text.len());
    out.push(ID_UNCONNECTED_PONG);
    out.extend_from_slice(&time.to_be_bytes());
    out.extend_from_slice(&server_guid.to_be_bytes());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&text[..usize::from(len)]);
    out
}

/// A decoded Unconnected Pong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pong {
    /// The timestamp from the ping.
    pub time: i64,
    /// The server's GUID.
    pub guid: i64,
    /// The status string (see [`Status`]).
    pub status: String,
}

/// Decodes an Unconnected Pong.
pub fn decode_pong(data: &[u8]) -> Option<Pong> {
    if data.len() < 35 || data[0] != ID_UNCONNECTED_PONG || data[17..33] != MAGIC {
        return None;
    }
    let time = i64::from_be_bytes(data[1..9].try_into().ok()?);
    let guid = i64::from_be_bytes(data[9..17].try_into().ok()?);
    let len = usize::from(u16::from_be_bytes([data[33], data[34]]));
    let text = data.get(35..35 + len)?;
    Some(Pong {
        time,
        guid,
        status: String::from_utf8_lossy(text).into_owned(),
    })
}

/// The fields of a pong's status string, which looks like
/// `MCPE;Dedicated Server;860;1.21.50;0;10;1234567890;Bedrock level;Survival;1;19132;19133`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// `MCPE` (or `MCEE` for Education).
    pub edition: String,
    /// The first line of the server's name.
    pub motd: String,
    /// Network protocol version.
    pub protocol: String,
    /// Game version, e.g. `1.21.50`.
    pub version: String,
    /// Players online.
    pub online: u32,
    /// Player limit.
    pub max: u32,
    /// The server's unique ID.
    pub guid: String,
    /// What follows (level name, game mode, ports), kept as sent.
    pub rest: Vec<String>,
}

impl Status {
    /// Parses a status string; `None` if it isn't one.
    pub fn parse(text: &str) -> Option<Self> {
        let mut fields = text.split(';');
        let edition = fields.next().filter(|e| matches!(*e, "MCPE" | "MCEE"))?;
        let motd = fields.next()?;
        let protocol = fields.next()?;
        let version = fields.next()?;
        let online = fields.next()?.parse().ok()?;
        let max = fields.next()?.parse().ok()?;
        let guid = fields.next()?;
        Some(Self {
            edition: edition.to_owned(),
            motd: motd.to_owned(),
            protocol: protocol.to_owned(),
            version: version.to_owned(),
            online,
            max,
            guid: guid.to_owned(),
            rest: fields.map(str::to_owned).collect(),
        })
    }

    /// The string form, as a pong carries it.
    pub fn render(&self) -> String {
        let mut out = format!(
            "{};{};{};{};{};{};{}",
            self.edition, self.motd, self.protocol, self.version, self.online, self.max, self.guid
        );
        for field in &self.rest {
            out.push(';');
            out.push_str(field);
        }
        out
    }

    /// A status for a server that isn't answering: `motd` as its name, no
    /// players. Clients show it greyed out if the protocol doesn't match,
    /// so the last known protocol and version are kept when there are any.
    pub fn placeholder(motd: &str, last: Option<&Status>, max: u32) -> Self {
        match last {
            Some(last) => Self {
                motd: motd.to_owned(),
                online: 0,
                ..last.clone()
            },
            None => Self {
                edition: "MCPE".into(),
                motd: motd.to_owned(),
                protocol: "0".into(),
                version: String::new(),
                online: 0,
                max,
                guid: "0".into(),
                rest: Vec::new(),
            },
        }
    }
}

#[cfg(feature = "tokio")]
pub use client::{PingError, ping};

#[cfg(feature = "tokio")]
mod client {
    use std::net::SocketAddr;
    use std::time::Duration;

    use tokio::net::UdpSocket;

    use super::{MAX_DATAGRAM, Pong, decode_pong, encode_ping};

    /// Why a Bedrock ping failed.
    #[derive(Debug, thiserror::Error)]
    pub enum PingError {
        /// The server didn't answer in time.
        #[error("timed out")]
        Timeout,
        /// The answer wasn't an Unconnected Pong.
        #[error("not a Bedrock pong")]
        NotAPong,
        /// Socket error.
        #[error(transparent)]
        Io(#[from] std::io::Error),
    }

    /// Pings the Bedrock server at `addr`, as a client's server list would.
    pub async fn ping(addr: SocketAddr, timeout: Duration) -> Result<Pong, PingError> {
        let local: SocketAddr = if addr.is_ipv4() {
            ([0, 0, 0, 0], 0).into()
        } else {
            ([0u16; 8], 0).into()
        };
        let socket = UdpSocket::bind(local).await?;
        socket.connect(addr).await?;
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        socket.send(&encode_ping(started, 0x4e4e)).await?;
        let mut buf = vec![0u8; MAX_DATAGRAM];
        let received = tokio::time::timeout(timeout, socket.recv(&mut buf))
            .await
            .map_err(|_| PingError::Timeout)??;
        decode_pong(&buf[..received]).ok_or(PingError::NotAPong)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "MCPE;Dedicated Server;860;1.21.50;3;10;13253860892328930865;Bedrock level;Survival;1;19132;19133;";

    #[test]
    fn pings_are_recognised() {
        let ping = encode_ping(1234, 99);
        assert_eq!(ping.len(), 33);
        assert_eq!(classify(&ping), Packet::Ping { time: 1234 });
        let mut open = ping.clone();
        open[0] = ID_UNCONNECTED_PING_OPEN;
        assert_eq!(classify(&open), Packet::Ping { time: 1234 });
    }

    #[test]
    fn a_ping_needs_the_magic() {
        let mut ping = encode_ping(1, 2);
        ping[12] ^= 0xff;
        assert_eq!(classify(&ping), Packet::Other);
        assert_eq!(classify(&[]), Packet::Other);
        assert_eq!(classify(&[ID_UNCONNECTED_PING]), Packet::Other);
    }

    #[test]
    fn joins_are_recognised() {
        let mut join = vec![ID_OPEN_CONNECTION_REQUEST_1];
        join.extend_from_slice(&MAGIC);
        join.push(11);
        join.extend(std::iter::repeat_n(0, 400));
        assert_eq!(classify(&join), Packet::OpenConnection);
        assert_eq!(classify(&join[..10]), Packet::Other);
        // Part of a running session.
        assert_eq!(classify(&[0x84, 0, 0, 0, 0]), Packet::Other);
    }

    #[test]
    fn pongs_round_trip() {
        let bytes = encode_pong(77, -5, SAMPLE);
        let pong = decode_pong(&bytes).unwrap();
        assert_eq!(pong.time, 77);
        assert_eq!(pong.guid, -5);
        assert_eq!(pong.status, SAMPLE);
        assert!(decode_pong(&bytes[..bytes.len() - 3]).is_none());
        assert!(decode_pong(&encode_ping(1, 2)).is_none());
    }

    #[test]
    fn status_strings_parse_and_render() {
        let status = Status::parse(SAMPLE).unwrap();
        assert_eq!(status.motd, "Dedicated Server");
        assert_eq!((status.online, status.max), (3, 10));
        assert_eq!(status.version, "1.21.50");
        assert_eq!(status.render(), SAMPLE);
        assert!(Status::parse("hello").is_none());
        assert!(Status::parse("MCPE;a;b").is_none());
        assert!(
            Status::parse("MCPE;a;1;1.0;x;10;1").is_none(),
            "player count"
        );
    }

    #[test]
    fn placeholders_keep_the_last_versions() {
        let last = Status::parse(SAMPLE).unwrap();
        let sleeping = Status::placeholder("Survival 💤 Sleeping — join to wake", Some(&last), 10);
        assert_eq!(sleeping.online, 0);
        assert_eq!(sleeping.protocol, "860");
        assert!(sleeping.render().starts_with("MCPE;Survival 💤"));
        let blank = Status::placeholder("Survival", None, 20);
        assert_eq!((blank.online, blank.max), (0, 20));
        assert!(Status::parse(&blank.render()).is_some());
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn pings_a_server_over_udp() {
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let (n, from) = server.recv_from(&mut buf).await.unwrap();
            let Packet::Ping { time } = classify(&buf[..n]) else {
                panic!("not a ping");
            };
            server
                .send_to(&encode_pong(time, 1, SAMPLE), from)
                .await
                .unwrap();
        });
        let pong = ping(addr, std::time::Duration::from_secs(2)).await.unwrap();
        assert_eq!(pong.status, SAMPLE);
    }
}
