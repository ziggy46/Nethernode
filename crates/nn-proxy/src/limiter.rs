//! Per-IP connection rate limiting (token bucket).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

/// Tracked IPs before idle buckets are pruned.
const PRUNE_THRESHOLD: usize = 50_000;

/// A token bucket per client IP: `burst` connections at once, refilled at
/// `per_second`.
#[derive(Debug)]
pub struct IpRateLimiter {
    burst: f64,
    per_second: f64,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl IpRateLimiter {
    /// Creates a limiter.
    pub fn new(burst: u32, per_second: f64) -> Self {
        Self {
            burst: f64::from(burst.max(1)),
            per_second: per_second.max(0.0),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Takes a token for `ip`. Returns `false` if the IP is over its limit.
    pub fn allow(&self, ip: IpAddr) -> bool {
        self.allow_at(ip, Instant::now())
    }

    fn allow_at(&self, ip: IpAddr, now: Instant) -> bool {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if buckets.len() >= PRUNE_THRESHOLD {
            let full_after = self.burst / self.per_second.max(f64::MIN_POSITIVE);
            buckets
                .retain(|_, b| now.saturating_duration_since(b.updated).as_secs_f64() < full_after);
        }
        let bucket = buckets.entry(ip).or_insert(Bucket {
            tokens: self.burst,
            updated: now,
        });
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.per_second).min(self.burst);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn allows_burst_then_refills() {
        let limiter = IpRateLimiter::new(3, 2.0);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let other: IpAddr = "10.0.0.2".parse().unwrap();
        let t0 = Instant::now();
        assert!((0..3).all(|_| limiter.allow_at(ip, t0)));
        assert!(!limiter.allow_at(ip, t0));
        assert!(limiter.allow_at(other, t0), "IPs are independent");
        // 2 tokens/s: after 500 ms, one more.
        assert!(limiter.allow_at(ip, t0 + Duration::from_millis(500)));
        assert!(!limiter.allow_at(ip, t0 + Duration::from_millis(500)));
        // Refill caps at the burst size.
        let later = t0 + Duration::from_secs(60);
        assert!((0..3).all(|_| limiter.allow_at(ip, later)));
        assert!(!limiter.allow_at(ip, later));
    }
}
