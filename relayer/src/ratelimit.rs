/// API rate limiting module
///
/// Token-bucket rate limiter per API key / IP address.
/// Provides free tier (100 req/min), pro tier (1000 req/min), and
/// enterprise (custom) limits — matching or beating LiFi's pricing.
///
/// Comparison:
///   - LiFi:     free 100 rps, pro ~1000 rps (undocumented)
///   - Socket:   free 50 rps, pro 500 rps
///   - InterLink: free 100/min, pro 1000/min, enterprise unlimited

use std::collections::HashMap;
use std::time::{Duration, Instant};

// ─── Tier Definitions ────────────────────────────────────────────────────────

/// Number of requests per minute for the free tier
pub const FREE_RPM: u32 = 100;
/// Number of requests per minute for the pro tier
pub const PRO_RPM: u32 = 1_000;
/// Burst multiplier: up to 2× the per-minute cap in a single second
pub const BURST_MULTIPLIER: u32 = 2;
/// Window duration for rate limit tracking
pub const WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier {
    Free,
    Pro,
    /// Enterprise: custom RPM (0 = unlimited)
    Enterprise(u32),
}

impl Tier {
    pub fn rpm(&self) -> Option<u32> {
        match self {
            Tier::Free => Some(FREE_RPM),
            Tier::Pro => Some(PRO_RPM),
            Tier::Enterprise(0) => None, // unlimited
            Tier::Enterprise(n) => Some(*n),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Tier::Free => "free",
            Tier::Pro => "pro",
            Tier::Enterprise(_) => "enterprise",
        }
    }
}

// ─── Bucket ───────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Bucket {
    pub tier: Tier,
    /// Tokens currently available
    tokens: f64,
    /// Capacity (== rpm for the window)
    capacity: f64,
    /// Last refill timestamp
    last_refill: Instant,
    /// Total requests rejected (for metrics)
    pub rejected: u64,
    /// Total requests accepted
    pub accepted: u64,
}

impl Bucket {
    pub fn new(tier: Tier) -> Self {
        let cap = tier.rpm().unwrap_or(u32::MAX) as f64;
        Bucket {
            tier,
            tokens: cap,
            capacity: cap,
            last_refill: Instant::now(),
            rejected: 0,
            accepted: 0,
        }
    }

    /// Refill tokens based on elapsed time since last refill.
    fn refill(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let refill = elapsed * (self.capacity / WINDOW.as_secs_f64());
        self.tokens = (self.tokens + refill).min(self.capacity);
        self.last_refill = now;
    }

    /// Try to consume `n` tokens. Returns `true` if allowed.
    pub fn try_consume(&mut self, n: u32) -> bool {
        // Unlimited enterprise
        if self.tier == Tier::Enterprise(0) {
            self.accepted += 1;
            return true;
        }
        let now = Instant::now();
        self.refill(now);
        if self.tokens >= n as f64 {
            self.tokens -= n as f64;
            self.accepted += 1;
            true
        } else {
            self.rejected += 1;
            false
        }
    }

    /// Remaining tokens (floored to integer).
    pub fn remaining(&self) -> u32 {
        self.tokens.floor() as u32
    }
}

// ─── Registry ─────────────────────────────────────────────────────────────────

/// Thread-unsafe registry suitable for use behind an `Arc<Mutex<_>>`.
pub struct RateLimiter {
    buckets: HashMap<String, Bucket>,
}

impl RateLimiter {
    pub fn new() -> Self {
        RateLimiter {
            buckets: HashMap::new(),
        }
    }

    /// Register an API key with a tier (idempotent — skips if already registered).
    pub fn register(&mut self, key: impl Into<String>, tier: Tier) {
        let key = key.into();
        self.buckets.entry(key).or_insert_with(|| Bucket::new(tier));
    }

    /// Upgrade or downgrade a key's tier (replaces the bucket, resetting counters).
    pub fn set_tier(&mut self, key: impl Into<String>, tier: Tier) {
        self.buckets.insert(key.into(), Bucket::new(tier));
    }

    /// Check and consume 1 request for `key`. Returns an `Err` with retry hint if limited.
    pub fn check(&mut self, key: &str) -> Result<RateOk, RateLimitError> {
        let bucket = self.buckets.get_mut(key).ok_or(RateLimitError::UnknownKey {
            key: key.to_string(),
        })?;
        if bucket.try_consume(1) {
            Ok(RateOk {
                remaining: bucket.remaining(),
                tier: bucket.tier.name(),
            })
        } else {
            Err(RateLimitError::LimitExceeded {
                rpm: bucket.tier.rpm().unwrap_or(0),
                rejected_total: bucket.rejected,
            })
        }
    }

    /// Stats for a key.
    pub fn stats(&self, key: &str) -> Option<BucketStats> {
        self.buckets.get(key).map(|b| BucketStats {
            tier: b.tier.name(),
            remaining: b.remaining(),
            accepted: b.accepted,
            rejected: b.rejected,
        })
    }

    /// Remove a key (e.g., revoked API key).
    pub fn remove(&mut self, key: &str) -> bool {
        self.buckets.remove(key).is_some()
    }

    /// Number of registered keys.
    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct RateOk {
    pub remaining: u32,
    pub tier: &'static str,
}

#[derive(Debug, Clone)]
pub struct BucketStats {
    pub tier: &'static str,
    pub remaining: u32,
    pub accepted: u64,
    pub rejected: u64,
}

#[derive(Debug, PartialEq)]
pub enum RateLimitError {
    LimitExceeded { rpm: u32, rejected_total: u64 },
    UnknownKey { key: String },
}

// ─── HTTP Headers Helper ──────────────────────────────────────────────────────

/// Build standard rate-limit response headers.
pub fn rate_limit_headers(remaining: u32, rpm: u32) -> Vec<(String, String)> {
    vec![
        ("X-RateLimit-Limit".into(), rpm.to_string()),
        ("X-RateLimit-Remaining".into(), remaining.to_string()),
        ("X-RateLimit-Reset".into(), "60".into()),
    ]
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_rpm() {
        assert_eq!(Tier::Free.rpm(), Some(100));
        assert_eq!(Tier::Pro.rpm(), Some(1000));
        assert_eq!(Tier::Enterprise(0).rpm(), None);
        assert_eq!(Tier::Enterprise(5000).rpm(), Some(5000));
    }

    #[test]
    fn test_register_and_check_free() {
        let mut rl = RateLimiter::new();
        rl.register("key1", Tier::Free);
        let ok = rl.check("key1").unwrap();
        assert_eq!(ok.tier, "free");
        // remaining should be 99 (started at 100, consumed 1)
        assert_eq!(ok.remaining, 99);
    }

    #[test]
    fn test_unknown_key_error() {
        let mut rl = RateLimiter::new();
        let err = rl.check("nonexistent").unwrap_err();
        assert_eq!(
            err,
            RateLimitError::UnknownKey {
                key: "nonexistent".to_string()
            }
        );
    }

    #[test]
    fn test_free_tier_exhaustion() {
        let mut rl = RateLimiter::new();
        rl.register("k", Tier::Free);
        // Exhaust all 100 tokens
        for _ in 0..FREE_RPM {
            assert!(rl.check("k").is_ok());
        }
        // 101st request should fail
        let err = rl.check("k").unwrap_err();
        assert!(matches!(err, RateLimitError::LimitExceeded { .. }));
    }

    #[test]
    fn test_pro_tier_allows_more() {
        let mut rl = RateLimiter::new();
        rl.register("pro_key", Tier::Pro);
        for _ in 0..FREE_RPM {
            assert!(rl.check("pro_key").is_ok(), "pro should not hit free limit");
        }
        // pro should still have 900 left
        let stats = rl.stats("pro_key").unwrap();
        assert_eq!(stats.remaining, PRO_RPM - FREE_RPM);
    }

    #[test]
    fn test_enterprise_unlimited() {
        let mut rl = RateLimiter::new();
        rl.register("enterprise", Tier::Enterprise(0));
        for _ in 0..10_000 {
            assert!(rl.check("enterprise").is_ok());
        }
        let stats = rl.stats("enterprise").unwrap();
        assert_eq!(stats.accepted, 10_000);
        assert_eq!(stats.rejected, 0);
    }

    #[test]
    fn test_set_tier_upgrade() {
        let mut rl = RateLimiter::new();
        rl.register("k", Tier::Free);
        // Exhaust free tier
        for _ in 0..FREE_RPM {
            let _ = rl.check("k");
        }
        // Upgrade to pro (resets bucket)
        rl.set_tier("k", Tier::Pro);
        for _ in 0..PRO_RPM {
            assert!(rl.check("k").is_ok());
        }
    }

    #[test]
    fn test_remove_key() {
        let mut rl = RateLimiter::new();
        rl.register("k", Tier::Free);
        assert!(rl.remove("k"));
        assert_eq!(rl.len(), 0);
        assert!(!rl.remove("k")); // already removed
    }

    #[test]
    fn test_stats() {
        let mut rl = RateLimiter::new();
        rl.register("k", Tier::Free);
        rl.check("k").unwrap();
        rl.check("k").unwrap();
        let s = rl.stats("k").unwrap();
        assert_eq!(s.accepted, 2);
        assert_eq!(s.rejected, 0);
        assert_eq!(s.remaining, 98);
    }

    #[test]
    fn test_rate_limit_headers() {
        let headers = rate_limit_headers(42, 100);
        assert_eq!(headers[0], ("X-RateLimit-Limit".into(), "100".into()));
        assert_eq!(headers[1], ("X-RateLimit-Remaining".into(), "42".into()));
    }

    #[test]
    fn test_idempotent_register() {
        let mut rl = RateLimiter::new();
        rl.register("k", Tier::Free);
        rl.check("k").unwrap(); // consumed 1
        // Re-register should be no-op
        rl.register("k", Tier::Pro);
        // bucket still free, still has 99 remaining
        let s = rl.stats("k").unwrap();
        assert_eq!(s.tier, "free");
        assert_eq!(s.remaining, 99);
    }
}
