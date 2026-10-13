//! PROXY protocol v2 headers, so backends see the player's real address.
//!
//! Backends must be configured to expect the header on *every* connection
//! (Paper: `proxies.proxy-protocol: true` in `config/paper-global.yml`;
//! Velocity: `haproxy-protocol = true`), or connections fail.
//! Spec: <https://www.haproxy.org/download/2.9/doc/proxy-protocol.txt> §2.2

use std::net::{IpAddr, SocketAddr};

const SIGNATURE: [u8; 12] = [
    0x0d, 0x0a, 0x0d, 0x0a, 0x00, 0x0d, 0x0a, 0x51, 0x55, 0x49, 0x54, 0x0a,
];
const VERSION_PROXY: u8 = 0x21;
const VERSION_LOCAL: u8 = 0x20;
const TCP_V4: u8 = 0x11;
const TCP_V6: u8 = 0x21;

/// A header describing a proxied connection from `client` to `local` (the
/// address the client connected to).
pub fn proxied(client: SocketAddr, local: SocketAddr) -> Vec<u8> {
    let mut out = Vec::with_capacity(52);
    out.extend_from_slice(&SIGNATURE);
    out.push(VERSION_PROXY);
    match (as_v4(client.ip()), as_v4(local.ip())) {
        (Some(src), Some(dst)) => {
            out.push(TCP_V4);
            out.extend_from_slice(&12u16.to_be_bytes());
            out.extend_from_slice(&src.octets());
            out.extend_from_slice(&dst.octets());
        }
        _ => {
            out.push(TCP_V6);
            out.extend_from_slice(&36u16.to_be_bytes());
            out.extend_from_slice(&as_v6(client.ip()).octets());
            out.extend_from_slice(&as_v6(local.ip()).octets());
        }
    }
    out.extend_from_slice(&client.port().to_be_bytes());
    out.extend_from_slice(&local.port().to_be_bytes());
    out
}

/// A `LOCAL` header, for connections the proxy makes itself (health checks).
pub fn local() -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(&SIGNATURE);
    out.push(VERSION_LOCAL);
    out.push(0x00);
    out.extend_from_slice(&0u16.to_be_bytes());
    out
}

fn as_v4(ip: IpAddr) -> Option<std::net::Ipv4Addr> {
    match ip {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped(),
    }
}

fn as_v6(ip: IpAddr) -> std::net::Ipv6Addr {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_ipv4() {
        let h = proxied(
            "203.0.113.7:54321".parse().unwrap(),
            "192.0.2.1:25565".parse().unwrap(),
        );
        assert_eq!(&h[..12], &SIGNATURE);
        assert_eq!(&h[12..16], &[0x21, 0x11, 0x00, 0x0c]);
        assert_eq!(&h[16..20], &[203, 0, 113, 7]);
        assert_eq!(&h[20..24], &[192, 0, 2, 1]);
        assert_eq!(&h[24..26], &54321u16.to_be_bytes());
        assert_eq!(&h[26..28], &25565u16.to_be_bytes());
        assert_eq!(h.len(), 28);
    }

    #[test]
    fn mapped_ipv4_on_a_dual_stack_socket_is_sent_as_ipv4() {
        let h = proxied(
            "[::ffff:203.0.113.7]:1".parse().unwrap(),
            "[::ffff:192.0.2.1]:2".parse().unwrap(),
        );
        assert_eq!(h[13], TCP_V4);
    }

    #[test]
    fn encodes_ipv6_and_mixed_families() {
        let h = proxied(
            "[2001:db8::1]:1000".parse().unwrap(),
            "[2001:db8::2]:25565".parse().unwrap(),
        );
        assert_eq!(&h[12..16], &[0x21, 0x21, 0x00, 0x24]);
        assert_eq!(h.len(), 16 + 36);
        let mixed = proxied(
            "[2001:db8::1]:1000".parse().unwrap(),
            "10.0.0.1:25565".parse().unwrap(),
        );
        assert_eq!(mixed[13], TCP_V6);
    }

    #[test]
    fn encodes_local() {
        assert_eq!(
            local(),
            [&SIGNATURE[..], &[0x20, 0x00, 0x00, 0x00]].concat()
        );
    }
}
