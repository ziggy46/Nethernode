//! Login rate limiting with exponential backoff.
//!
//! Failures are counted separately per client IP and per username. After a
//! key's free attempts are used up, each further failure locks the key for
//! `base_delay * 2^(n - 1)`, capped at `max_delay`. A key's failure count is
//! forgotten after `reset_after` passes without a failure.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Tuning for one dimension (IP or username) of the limiter.
#[derive(Debug, Clone, Copy)]
pub struct BackoffPolicy {
    /// Failures allowed before backoff starts.
    pub free_attempts: u32,
    /// Lockout after the first failure past the free attempts.
    pub base_delay: Duration,
    /// Upper bound on any single lockout.
    pub max_delay: Duration,
    /// Quiet period after which the failure count resets.
    pub reset_after: Duration,
}

impl BackoffPolicy {
    /// Lockout duration after `failures` total failures.
    fn delay_for(&self, failures: u32) -> Option<Duration> {
        let over = failures.checked_sub(self.free_attempts)?;
        if over == 0 {
            return None;
        }
        let factor = 1u32.checked_shl(over - 1).unwrap_or(u32::MAX);
        Some(self.base_delay.saturating_mul(factor).min(self.max_delay))
    }
}

/// Default per-username policy: 5 free attempts, then 1 s, 2 s, 4 s… up to 15 min.
pub const USERNAME_POLICY: BackoffPolicy = BackoffPolicy {
    free_attempts: 5,
    base_delay: Duration::from_secs(1),
    max_delay: Duration::from_secs(15 * 60),
    reset_after: Duration::from_secs(60 * 60),
};

/// Default per-IP policy: 20 free attempts, then 1 s, 2 s, 4 s… up to 15 min.
pub const IP_POLICY: BackoffPolicy = BackoffPolicy {
    free_attempts: 20,
    base_delay: Duration::from_secs(1),
    max_delay: Duration::from_secs(15 * 60),
    reset_after: Duration::from_secs(60 * 60),
};

/// Maximum tracked keys per dimension before stale entries are pruned.
const PRUNE_THRESHOLD: usize = 10_000;

#[derive(Debug, Clone, Copy)]
struct Entry {
    failures: u32,
    last_failure: Instant,
    locked_until: Option<Instant>,
}

#[derive(Debug)]
struct Bucket {
    policy: BackoffPolicy,
    entries: Mutex<HashMap<String, Entry>>,
}

impl Bucket {
    fn new(policy: BackoffPolicy) -> Self {
        Self {
            policy,
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn retry_after(&self, key: &str, now: Instant) -> Option<Duration> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let until = entries.get(key)?.locked_until?;
        until.checked_duration_since(now).filter(|d| !d.is_zero())
    }

    fn record_failure(&self, key: &str, now: Instant) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= PRUNE_THRESHOLD {
            let reset_after = self.policy.reset_after;
            entries.retain(|_, e| now.saturating_duration_since(e.last_failure) < reset_after);
        }
        let entry = entries.entry(key.to_owned()).or_insert(Entry {
            failures: 0,
            last_failure: now,
            locked_until: None,
        });
        if now.saturating_duration_since(entry.last_failure) >= self.policy.reset_after {
            entry.failures = 0;
        }
        entry.failures = entry.failures.saturating_add(1);
        entry.last_failure = now;
        entry.locked_until = self.policy.delay_for(entry.failures).map(|d| now + d);
    }

    fn clear(&self, key: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }
}

/// Tracks failed logins per IP and per username.
#[derive(Debug)]
pub struct LoginRateLimiter {
    by_ip: Bucket,
    by_username: Bucket,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new(IP_POLICY, USERNAME_POLICY)
    }
}

impl LoginRateLimiter {
    /// Creates a limiter with the given per-IP and per-username policies.
    pub fn new(ip: BackoffPolicy, username: BackoffPolicy) -> Self {
        Self {
            by_ip: Bucket::new(ip),
            by_username: Bucket::new(username),
        }
    }

    /// Returns how long the caller must wait before trying again, if locked out.
    pub fn check(&self, ip: Option<&str>, username: &str) -> Option<Duration> {
        self.check_at(ip, username, Instant::now())
    }

    /// Records a failed attempt.
    pub fn record_failure(&self, ip: Option<&str>, username: &str) {
        self.record_failure_at(ip, username, Instant::now());
    }

    /// Records a successful login, clearing the username's failures.
    ///
    /// The IP's count is left alone so one valid account can't be used to
    /// reset an IP that is guessing at others.
    pub fn record_success(&self, username: &str) {
        self.by_username.clear(&normalize(username));
    }

    fn check_at(&self, ip: Option<&str>, username: &str, now: Instant) -> Option<Duration> {
        let by_ip = ip.and_then(|ip| self.by_ip.retry_after(ip, now));
        let by_user = self.by_username.retry_after(&normalize(username), now);
        by_ip.max(by_user)
    }

    fn record_failure_at(&self, ip: Option<&str>, username: &str, now: Instant) {
        if let Some(ip) = ip {
            self.by_ip.record_failure(ip, now);
        }
        self.by_username.record_failure(&normalize(username), now);
    }
}

fn normalize(username: &str) -> String {
    username.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: BackoffPolicy = BackoffPolicy {
        free_attempts: 3,
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(8),
        reset_after: Duration::from_secs(100),
    };

    #[test]
    fn delay_grows_exponentially_and_caps() {
        let delays: Vec<_> = (1..=8)
            .map(|n| POLICY.delay_for(n).map(|d| d.as_secs()))
            .collect();
        assert_eq!(
            delays,
            [
                None,
                None,
                None,
                Some(1),
                Some(2),
                Some(4),
                Some(8),
                Some(8)
            ]
        );
        assert_eq!(POLICY.delay_for(u32::MAX), Some(Duration::from_secs(8)));
    }

    #[test]
    fn username_locks_after_free_attempts() {
        let limiter = LoginRateLimiter::new(USERNAME_POLICY, POLICY);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert_eq!(limiter.check_at(None, "Alice", t0), None);
            limiter.record_failure_at(None, "Alice", t0);
        }
        // Fourth failure triggers a 1 s lock, case-insensitively.
        limiter.record_failure_at(None, "alice", t0);
        assert_eq!(
            limiter.check_at(None, "ALICE", t0),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            limiter.check_at(None, "alice", t0 + Duration::from_secs(1)),
            None
        );
        // Other usernames are unaffected.
        assert_eq!(limiter.check_at(None, "bob", t0), None);
    }

    #[test]
    fn ip_locks_across_usernames() {
        let limiter = LoginRateLimiter::new(POLICY, USERNAME_POLICY);
        let t0 = Instant::now();
        for (i, name) in ["a", "b", "c", "d"].iter().enumerate() {
            assert_eq!(
                limiter.check_at(Some("10.0.0.1"), name, t0),
                None,
                "attempt {i}"
            );
            limiter.record_failure_at(Some("10.0.0.1"), name, t0);
        }
        assert!(limiter.check_at(Some("10.0.0.1"), "e", t0).is_some());
        assert!(limiter.check_at(Some("10.0.0.2"), "e", t0).is_none());
    }

    #[test]
    fn success_clears_username_but_not_ip() {
        let limiter = LoginRateLimiter::new(POLICY, POLICY);
        let t0 = Instant::now();
        for _ in 0..4 {
            limiter.record_failure_at(Some("ip"), "alice", t0);
        }
        limiter.record_success("alice");
        assert_eq!(limiter.check_at(None, "alice", t0), None);
        assert!(limiter.check_at(Some("ip"), "alice", t0).is_some());
    }

    #[test]
    fn failures_reset_after_quiet_period() {
        let limiter = LoginRateLimiter::new(USERNAME_POLICY, POLICY);
        let t0 = Instant::now();
        for _ in 0..3 {
            limiter.record_failure_at(None, "alice", t0);
        }
        let later = t0 + Duration::from_secs(100);
        limiter.record_failure_at(None, "alice", later);
        assert_eq!(limiter.check_at(None, "alice", later), None);
    }
}
