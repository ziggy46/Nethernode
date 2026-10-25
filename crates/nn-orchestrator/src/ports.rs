//! Host port allocation for game and RCON ports.

use std::collections::HashSet;
use std::ops::RangeInclusive;

/// Picks `count` free ports from `range`, lowest first, skipping ports in
/// `used` and any for which `is_free` says no (e.g. taken by another program).
pub fn allocate(
    range: RangeInclusive<u16>,
    used: &[i64],
    count: usize,
    is_free: impl Fn(u16) -> bool,
) -> Option<Vec<u16>> {
    let used: HashSet<i64> = used.iter().copied().collect();
    let free: Vec<u16> = range
        .filter(|p| !used.contains(&i64::from(*p)) && is_free(*p))
        .take(count)
        .collect();
    (free.len() == count).then_some(free)
}

/// Whether nothing on this host is listening on `127.0.0.1:port`.
pub fn is_free_on_localhost(port: u16) -> bool {
    is_free(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port)
}

/// Whether nothing is listening on `ip:port`. An address that isn't this
/// machine's (the Docker host's bridge gateway, seen from a container)
/// can't be checked; Docker refuses a taken port when publishing anyway.
pub fn is_free(ip: std::net::IpAddr, port: u16) -> bool {
    match std::net::TcpListener::bind((ip, port)) {
        Ok(_) => true,
        Err(e) => e.kind() == std::io::ErrorKind::AddrNotAvailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_lowest_free_ports() {
        let any = |_| true;
        assert_eq!(
            allocate(30000..=30999, &[], 2, any),
            Some(vec![30000, 30001])
        );
        assert_eq!(
            allocate(30000..=30999, &[30000, 30002], 2, any),
            Some(vec![30001, 30003])
        );
        // Ports busy on the host are skipped too.
        assert_eq!(
            allocate(30000..=30999, &[], 2, |p| p != 30001),
            Some(vec![30000, 30002])
        );
    }

    #[test]
    fn fails_when_exhausted() {
        assert_eq!(allocate(30000..=30001, &[30000], 2, |_| true), None);
        assert_eq!(
            allocate(30000..=30001, &[], 2, |_| true),
            Some(vec![30000, 30001])
        );
    }

    #[test]
    fn detects_busy_ports() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(!is_free_on_localhost(held.local_addr().unwrap().port()));
    }
}
