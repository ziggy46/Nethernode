//! Host port allocation for game and RCON ports.

use std::collections::HashSet;
use std::ops::RangeInclusive;

/// Picks `count` free ports from `range`, lowest first, skipping `used`.
pub fn allocate(range: RangeInclusive<u16>, used: &[i64], count: usize) -> Option<Vec<u16>> {
    let used: HashSet<i64> = used.iter().copied().collect();
    let free: Vec<u16> = range
        .filter(|p| !used.contains(&i64::from(*p)))
        .take(count)
        .collect();
    (free.len() == count).then_some(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_lowest_free_ports() {
        assert_eq!(allocate(30000..=30999, &[], 2), Some(vec![30000, 30001]));
        assert_eq!(
            allocate(30000..=30999, &[30000, 30002], 2),
            Some(vec![30001, 30003])
        );
    }

    #[test]
    fn fails_when_exhausted() {
        assert_eq!(allocate(30000..=30001, &[30000], 2), None);
        assert_eq!(allocate(30000..=30001, &[], 2), Some(vec![30000, 30001]));
    }
}
