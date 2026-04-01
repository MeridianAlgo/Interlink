/// Production-grade retry and resilience engine for InterLink
///
/// Exponential backoff with jitter, per-chain retry policies,
/// circuit-breaker-aware retries, and dead-letter queue for failed operations.
///
/// Features:
///   - Exponential backoff: base_delay * 2^attempt + random jitter
///   - Per-chain policies: Ethereum (slower, more retries), Solana (faster, fewer)
///   - Circuit-breaker integration: skip retries when bridge is paused
///   - Dead-letter queue: failed ops stored for manual review
///   - Budget tracking: max retries + max total time per operation
///
/// Comparison:
///   Wormhole:  guardians retry internally, no configurable policy
///   Across:    relayer retry is opaque
///   InterLink: fully configurable retry with observability + dead-letter

use std::collections::VecDeque;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Default base delay in milliseconds.
pub const DEFAULT_BASE_DELAY_MS: u64 = 500;
/// Default maximum delay cap in milliseconds (30 seconds).
pub const DEFAULT_MAX_DELAY_MS: u64 = 30_000;
/// Default maximum retry attempts.
pub const DEFAULT_MAX_RETRIES: u32 = 5;
/// Default jitter factor (0.0–1.0).
pub const DEFAULT_JITTER_FACTOR: f64 = 0.25;
/// Default total timeout in milliseconds (5 minutes).
pub const DEFAULT_TOTAL_TIMEOUT_MS: u64 = 300_000;
/// Dead-letter queue max size.
pub const DLQ_MAX_SIZE: usize = 1_000;

// ─── Chain-specific presets ─────────────────────────────────────────────────

/// Ethereum: slow blocks, more retries needed.
pub const ETH_BASE_DELAY_MS: u64 = 2_000;
pub const ETH_MAX_DELAY_MS: u64 = 60_000;
pub const ETH_MAX_RETRIES: u32 = 8;

/// Solana: fast blocks, quick retries.
pub const SOL_BASE_DELAY_MS: u64 = 200;
pub const SOL_MAX_DELAY_MS: u64 = 5_000;
pub const SOL_MAX_RETRIES: u32 = 4;

/// L2 chains (Optimism, Arbitrum, Base): moderate.
pub const L2_BASE_DELAY_MS: u64 = 500;
pub const L2_MAX_DELAY_MS: u64 = 15_000;
pub const L2_MAX_RETRIES: u32 = 5;

// ─── Types ──────────────────────────────────────────────────────────────────

/// Retry policy configuration.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Base delay between retries (ms).
    pub base_delay_ms: u64,
    /// Maximum delay cap (ms).
    pub max_delay_ms: u64,
    /// Maximum retry attempts.
    pub max_retries: u32,
    /// Jitter factor (0.0 = no jitter, 1.0 = full jitter).
    pub jitter_factor: f64,
    /// Total timeout for all retries (ms).
    pub total_timeout_ms: u64,
    /// Whether to respect circuit breaker state.
    pub circuit_breaker_aware: bool,
}

impl RetryPolicy {
    /// Default retry policy.
    pub fn default_policy() -> Self {
        RetryPolicy {
            base_delay_ms: DEFAULT_BASE_DELAY_MS,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
            max_retries: DEFAULT_MAX_RETRIES,
            jitter_factor: DEFAULT_JITTER_FACTOR,
            total_timeout_ms: DEFAULT_TOTAL_TIMEOUT_MS,
            circuit_breaker_aware: true,
        }
    }

    /// Ethereum-optimized retry policy.
    pub fn ethereum() -> Self {
        RetryPolicy {
            base_delay_ms: ETH_BASE_DELAY_MS,
            max_delay_ms: ETH_MAX_DELAY_MS,
            max_retries: ETH_MAX_RETRIES,
            jitter_factor: DEFAULT_JITTER_FACTOR,
            total_timeout_ms: 600_000, // 10 min for Ethereum
            circuit_breaker_aware: true,
        }
    }

    /// Solana-optimized retry policy.
    pub fn solana() -> Self {
        RetryPolicy {
            base_delay_ms: SOL_BASE_DELAY_MS,
            max_delay_ms: SOL_MAX_DELAY_MS,
            max_retries: SOL_MAX_RETRIES,
            jitter_factor: DEFAULT_JITTER_FACTOR,
            total_timeout_ms: 30_000, // 30s for Solana
            circuit_breaker_aware: true,
        }
    }

    /// L2-optimized retry policy.
    pub fn l2() -> Self {
        RetryPolicy {
            base_delay_ms: L2_BASE_DELAY_MS,
            max_delay_ms: L2_MAX_DELAY_MS,
            max_retries: L2_MAX_RETRIES,
            jitter_factor: DEFAULT_JITTER_FACTOR,
            total_timeout_ms: 120_000, // 2 min for L2
            circuit_breaker_aware: true,
        }
    }

    /// Get the chain-appropriate retry policy.
    pub fn for_chain(chain_id: u32) -> Self {
        match chain_id {
            1 => Self::ethereum(),         // Ethereum mainnet
            900 => Self::solana(),          // Solana
            10 | 42161 | 8453 => Self::l2(), // Optimism, Arbitrum, Base
            137 => {                        // Polygon PoS
                let mut p = Self::l2();
                p.base_delay_ms = 1_000;   // Polygon checkpoints are slower
                p.max_retries = 6;
                p
            }
            _ => Self::default_policy(),
        }
    }

    /// Compute delay for attempt N (0-indexed) with deterministic seed.
    pub fn delay_for_attempt(&self, attempt: u32, jitter_seed: u64) -> u64 {
        let exp_delay = self.base_delay_ms.saturating_mul(1u64 << attempt.min(16));
        let capped = exp_delay.min(self.max_delay_ms);
        // Deterministic jitter using seed
        let jitter_range = (capped as f64 * self.jitter_factor) as u64;
        if jitter_range == 0 {
            return capped;
        }
        let jitter = jitter_seed % (jitter_range * 2 + 1);
        let jitter_signed = jitter as i64 - jitter_range as i64;
        (capped as i64 + jitter_signed).max(0) as u64
    }
}

/// Outcome of a retry attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryOutcome {
    /// Operation succeeded on this attempt.
    Success { attempt: u32 },
    /// Operation failed but will be retried.
    RetryScheduled { attempt: u32, delay_ms: u64 },
    /// All retries exhausted.
    Exhausted { attempts: u32 },
    /// Circuit breaker is open — don't retry.
    CircuitOpen,
    /// Total timeout exceeded.
    TimedOut { elapsed_ms: u64 },
}

/// A failed operation in the dead-letter queue.
#[derive(Debug, Clone)]
pub struct DeadLetterEntry {
    /// Operation identifier.
    pub operation_id: String,
    /// Chain ID where the operation failed.
    pub chain_id: u32,
    /// Number of attempts made.
    pub attempts: u32,
    /// Last error message.
    pub last_error: String,
    /// Timestamp when the operation was moved to DLQ.
    pub failed_at: u64,
    /// The serialized operation payload (for replay).
    pub payload: Vec<u8>,
}

// ─── Retry Engine ───────────────────────────────────────────────────────────

pub struct RetryEngine {
    /// Dead-letter queue for permanently failed operations.
    dead_letters: VecDeque<DeadLetterEntry>,
    /// Retry statistics.
    total_retries: u64,
    total_successes: u64,
    total_exhausted: u64,
}

impl RetryEngine {
    pub fn new() -> Self {
        RetryEngine {
            dead_letters: VecDeque::new(),
            total_retries: 0,
            total_successes: 0,
            total_exhausted: 0,
        }
    }

    /// Evaluate whether to retry an operation.
    pub fn evaluate(
        &mut self,
        policy: &RetryPolicy,
        attempt: u32,
        elapsed_ms: u64,
        circuit_open: bool,
        jitter_seed: u64,
    ) -> RetryOutcome {
        // Circuit breaker check
        if policy.circuit_breaker_aware && circuit_open {
            return RetryOutcome::CircuitOpen;
        }

        // Total timeout check
        if elapsed_ms >= policy.total_timeout_ms {
            self.total_exhausted += 1;
            return RetryOutcome::TimedOut { elapsed_ms };
        }

        // Max retries check
        if attempt >= policy.max_retries {
            self.total_exhausted += 1;
            return RetryOutcome::Exhausted { attempts: attempt };
        }

        // Schedule retry
        let delay = policy.delay_for_attempt(attempt, jitter_seed);
        self.total_retries += 1;
        RetryOutcome::RetryScheduled {
            attempt,
            delay_ms: delay,
        }
    }

    /// Record a successful operation.
    pub fn record_success(&mut self) {
        self.total_successes += 1;
    }

    /// Add a failed operation to the dead-letter queue.
    pub fn add_dead_letter(&mut self, entry: DeadLetterEntry) {
        if self.dead_letters.len() >= DLQ_MAX_SIZE {
            self.dead_letters.pop_front();
        }
        self.dead_letters.push_back(entry);
    }

    /// Get all dead-letter entries.
    pub fn dead_letters(&self) -> &VecDeque<DeadLetterEntry> {
        &self.dead_letters
    }

    /// Replay a dead-letter entry (remove from DLQ, return payload).
    pub fn replay_dead_letter(&mut self, operation_id: &str) -> Option<DeadLetterEntry> {
        if let Some(pos) = self
            .dead_letters
            .iter()
            .position(|e| e.operation_id == operation_id)
        {
            self.dead_letters.remove(pos)
        } else {
            None
        }
    }

    /// Statistics.
    pub fn stats(&self) -> RetryStats {
        RetryStats {
            total_retries: self.total_retries,
            total_successes: self.total_successes,
            total_exhausted: self.total_exhausted,
            dead_letter_count: self.dead_letters.len(),
        }
    }

    /// Stats as JSON.
    pub fn stats_json(&self) -> serde_json::Value {
        let s = self.stats();
        serde_json::json!({
            "total_retries": s.total_retries,
            "total_successes": s.total_successes,
            "total_exhausted": s.total_exhausted,
            "dead_letter_count": s.dead_letter_count,
            "success_rate": if s.total_retries + s.total_successes > 0 {
                s.total_successes as f64 / (s.total_retries + s.total_successes) as f64
            } else {
                1.0
            },
        })
    }
}

impl Default for RetryEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct RetryStats {
    pub total_retries: u64,
    pub total_successes: u64,
    pub total_exhausted: u64,
    pub dead_letter_count: usize,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_values() {
        let p = RetryPolicy::default_policy();
        assert_eq!(p.base_delay_ms, 500);
        assert_eq!(p.max_retries, 5);
        assert!(p.circuit_breaker_aware);
    }

    #[test]
    fn test_exponential_backoff() {
        let p = RetryPolicy {
            base_delay_ms: 100,
            max_delay_ms: 100_000,
            max_retries: 10,
            jitter_factor: 0.0, // no jitter for predictability
            total_timeout_ms: 600_000,
            circuit_breaker_aware: false,
        };
        assert_eq!(p.delay_for_attempt(0, 0), 100);   // 100 * 2^0
        assert_eq!(p.delay_for_attempt(1, 0), 200);   // 100 * 2^1
        assert_eq!(p.delay_for_attempt(2, 0), 400);   // 100 * 2^2
        assert_eq!(p.delay_for_attempt(3, 0), 800);   // 100 * 2^3
        assert_eq!(p.delay_for_attempt(4, 0), 1600);  // 100 * 2^4
    }

    #[test]
    fn test_delay_capped_at_max() {
        let p = RetryPolicy {
            base_delay_ms: 1000,
            max_delay_ms: 5000,
            max_retries: 10,
            jitter_factor: 0.0,
            total_timeout_ms: 600_000,
            circuit_breaker_aware: false,
        };
        assert_eq!(p.delay_for_attempt(0, 0), 1000);
        assert_eq!(p.delay_for_attempt(3, 0), 5000); // 8000 capped to 5000
        assert_eq!(p.delay_for_attempt(10, 0), 5000);
    }

    #[test]
    fn test_jitter_adds_variance() {
        let p = RetryPolicy {
            base_delay_ms: 1000,
            max_delay_ms: 100_000,
            max_retries: 10,
            jitter_factor: 0.5,
            total_timeout_ms: 600_000,
            circuit_breaker_aware: false,
        };
        let d1 = p.delay_for_attempt(2, 42);
        let d2 = p.delay_for_attempt(2, 99);
        // With different seeds, delays should differ (with high probability)
        // Both should be in range [4000 - 2000, 4000 + 2000] = [2000, 6000]
        assert!(d1 >= 2000 && d1 <= 6000, "d1={d1}");
        assert!(d2 >= 2000 && d2 <= 6000, "d2={d2}");
    }

    #[test]
    fn test_chain_policies() {
        let eth = RetryPolicy::for_chain(1);
        assert_eq!(eth.base_delay_ms, ETH_BASE_DELAY_MS);
        assert_eq!(eth.max_retries, ETH_MAX_RETRIES);

        let sol = RetryPolicy::for_chain(900);
        assert_eq!(sol.base_delay_ms, SOL_BASE_DELAY_MS);
        assert_eq!(sol.max_retries, SOL_MAX_RETRIES);

        let l2 = RetryPolicy::for_chain(10);
        assert_eq!(l2.base_delay_ms, L2_BASE_DELAY_MS);

        let polygon = RetryPolicy::for_chain(137);
        assert_eq!(polygon.base_delay_ms, 1000); // custom
    }

    #[test]
    fn test_evaluate_retry_scheduled() {
        let mut engine = RetryEngine::new();
        let policy = RetryPolicy::default_policy();
        let outcome = engine.evaluate(&policy, 0, 0, false, 0);
        match outcome {
            RetryOutcome::RetryScheduled { attempt, delay_ms } => {
                assert_eq!(attempt, 0);
                assert!(delay_ms > 0);
            }
            _ => panic!("expected RetryScheduled, got {outcome:?}"),
        }
    }

    #[test]
    fn test_evaluate_exhausted() {
        let mut engine = RetryEngine::new();
        let policy = RetryPolicy::default_policy();
        let outcome = engine.evaluate(&policy, policy.max_retries, 0, false, 0);
        assert_eq!(outcome, RetryOutcome::Exhausted { attempts: policy.max_retries });
    }

    #[test]
    fn test_evaluate_circuit_open() {
        let mut engine = RetryEngine::new();
        let policy = RetryPolicy::default_policy();
        let outcome = engine.evaluate(&policy, 0, 0, true, 0);
        assert_eq!(outcome, RetryOutcome::CircuitOpen);
    }

    #[test]
    fn test_evaluate_timed_out() {
        let mut engine = RetryEngine::new();
        let policy = RetryPolicy::default_policy();
        let outcome = engine.evaluate(&policy, 1, policy.total_timeout_ms, false, 0);
        match outcome {
            RetryOutcome::TimedOut { elapsed_ms } => assert_eq!(elapsed_ms, policy.total_timeout_ms),
            _ => panic!("expected TimedOut"),
        }
    }

    #[test]
    fn test_dead_letter_queue() {
        let mut engine = RetryEngine::new();
        engine.add_dead_letter(DeadLetterEntry {
            operation_id: "op1".into(),
            chain_id: 1,
            attempts: 5,
            last_error: "RPC timeout".into(),
            failed_at: 1000,
            payload: vec![1, 2, 3],
        });
        assert_eq!(engine.dead_letters().len(), 1);
        assert_eq!(engine.dead_letters()[0].operation_id, "op1");
    }

    #[test]
    fn test_replay_dead_letter() {
        let mut engine = RetryEngine::new();
        engine.add_dead_letter(DeadLetterEntry {
            operation_id: "op1".into(),
            chain_id: 1,
            attempts: 3,
            last_error: "timeout".into(),
            failed_at: 1000,
            payload: vec![4, 5, 6],
        });
        let entry = engine.replay_dead_letter("op1").unwrap();
        assert_eq!(entry.payload, vec![4, 5, 6]);
        assert!(engine.dead_letters().is_empty());
    }

    #[test]
    fn test_replay_nonexistent() {
        let mut engine = RetryEngine::new();
        assert!(engine.replay_dead_letter("nope").is_none());
    }

    #[test]
    fn test_dlq_eviction_at_max_size() {
        let mut engine = RetryEngine::new();
        for i in 0..DLQ_MAX_SIZE + 5 {
            engine.add_dead_letter(DeadLetterEntry {
                operation_id: format!("op{i}"),
                chain_id: 1,
                attempts: 1,
                last_error: "err".into(),
                failed_at: i as u64,
                payload: vec![],
            });
        }
        assert_eq!(engine.dead_letters().len(), DLQ_MAX_SIZE);
        // Oldest should have been evicted
        assert_eq!(engine.dead_letters()[0].operation_id, "op5");
    }

    #[test]
    fn test_stats_tracking() {
        let mut engine = RetryEngine::new();
        let policy = RetryPolicy::default_policy();
        engine.evaluate(&policy, 0, 0, false, 0);
        engine.evaluate(&policy, 1, 1000, false, 0);
        engine.record_success();
        engine.evaluate(&policy, policy.max_retries, 0, false, 0);
        let stats = engine.stats();
        assert_eq!(stats.total_retries, 2);
        assert_eq!(stats.total_successes, 1);
        assert_eq!(stats.total_exhausted, 1);
    }

    #[test]
    fn test_stats_json() {
        let engine = RetryEngine::new();
        let j = engine.stats_json();
        assert_eq!(j["total_retries"], 0);
        assert_eq!(j["success_rate"], 1.0);
    }

    #[test]
    fn test_circuit_breaker_not_aware_skips_check() {
        let mut engine = RetryEngine::new();
        let mut policy = RetryPolicy::default_policy();
        policy.circuit_breaker_aware = false;
        // Even with circuit open, should still schedule retry
        let outcome = engine.evaluate(&policy, 0, 0, true, 0);
        match outcome {
            RetryOutcome::RetryScheduled { .. } => {}
            _ => panic!("expected RetryScheduled when cb-unaware"),
        }
    }

    #[test]
    fn test_solana_fast_timeout() {
        let sol = RetryPolicy::solana();
        assert_eq!(sol.total_timeout_ms, 30_000);
        assert!(sol.total_timeout_ms < RetryPolicy::ethereum().total_timeout_ms);
    }
}
