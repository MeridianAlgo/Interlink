/// Circuit breaker / emergency pause system for InterLink (Phase 4)
///
/// Implements a production-grade circuit breaker that halts bridge operations
/// when anomalous conditions are detected:
///
///   - Consecutive proof failures exceed threshold → auto-pause
///   - Settlement failures exceed threshold → auto-pause
///   - Manual guardian pause (key rotation, security incident)
///   - TVL drain detection (large outflow in short window)
///   - Automatic cooldown + recovery
///
/// Comparison:
///   Wormhole:  Governor rate-limits large transfers, guardians can pause
///   Across:    UMA oracle dispute mechanism + manual pause
///   Nomad:     Had NO circuit breaker → lost $190M in 2022
///   InterLink: Automatic anomaly detection + guardian pause + TVL drain guard
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const RLX: Ordering = Ordering::Relaxed;

// ─── Configuration ───────────────────────────────────────────────────────────

/// Consecutive proof failures before auto-pause
pub const PROOF_FAILURE_THRESHOLD: u32 = 5;
/// Consecutive settlement failures before auto-pause
pub const SETTLEMENT_FAILURE_THRESHOLD: u32 = 3;
/// Maximum outflow (in cents) within the drain window before auto-pause
pub const TVL_DRAIN_LIMIT_CENTS: u64 = 100_000_000; // $1M in cents
/// Drain detection window in seconds
pub const DRAIN_WINDOW_SECS: u64 = 300; // 5 minutes
/// Cooldown period before auto-recovery in seconds
pub const COOLDOWN_SECS: u64 = 300; // 5 minutes
/// Maximum pause duration before requiring manual intervention (1 hour)
pub const MAX_PAUSE_DURATION_SECS: u64 = 3600;

// ─── Pause Reason ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PauseReason {
    /// Too many consecutive proof generation failures
    ProofFailures { count: u32 },
    /// Too many consecutive settlement failures
    SettlementFailures { count: u32 },
    /// Guardian manually triggered pause
    GuardianPause { guardian: String, message: String },
    /// TVL drain detected: large outflow in short window
    TvlDrain {
        outflow_cents: u64,
        window_secs: u64,
    },
    /// Validator set compromise detected
    ValidatorCompromise,
}

impl PauseReason {
    pub fn severity(&self) -> &'static str {
        match self {
            PauseReason::ProofFailures { .. } => "HIGH",
            PauseReason::SettlementFailures { .. } => "CRITICAL",
            PauseReason::GuardianPause { .. } => "MANUAL",
            PauseReason::TvlDrain { .. } => "CRITICAL",
            PauseReason::ValidatorCompromise => "EMERGENCY",
        }
    }

    pub fn auto_recoverable(&self) -> bool {
        match self {
            PauseReason::ProofFailures { .. } => true,
            PauseReason::SettlementFailures { .. } => true,
            PauseReason::GuardianPause { .. } => false,
            PauseReason::TvlDrain { .. } => false,
            PauseReason::ValidatorCompromise => false,
        }
    }
}

// ─── Outflow Record ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct OutflowRecord {
    amount_cents: u64,
    timestamp: u64,
}

// ─── Circuit Breaker State ───────────────────────────────────────────────────

#[derive(Debug)]
struct Inner {
    paused: AtomicBool,
    pause_reason: Mutex<Option<PauseReason>>,
    paused_at: AtomicU64,
    consecutive_proof_failures: AtomicU64,
    consecutive_settlement_failures: AtomicU64,
    total_pauses: AtomicU64,
    outflows: Mutex<VecDeque<OutflowRecord>>,
    /// Authorized guardian public key hashes (SHA-256 of pubkey bytes)
    guardians: Mutex<Vec<[u8; 32]>>,
}

/// Thread-safe circuit breaker handle.
#[derive(Clone, Debug)]
pub struct CircuitBreaker(Arc<Inner>);

impl CircuitBreaker {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            paused: AtomicBool::new(false),
            pause_reason: Mutex::new(None),
            paused_at: AtomicU64::new(0),
            consecutive_proof_failures: AtomicU64::new(0),
            consecutive_settlement_failures: AtomicU64::new(0),
            total_pauses: AtomicU64::new(0),
            outflows: Mutex::new(VecDeque::new()),
            guardians: Mutex::new(Vec::new()),
        }))
    }

    // ── Status ──────────────────────────────────────────────────────────────

    /// Whether the bridge is currently paused.
    pub fn is_paused(&self) -> bool {
        self.0.paused.load(RLX)
    }

    /// Whether the bridge is operational (not paused).
    pub fn is_operational(&self) -> bool {
        !self.is_paused()
    }

    /// Current pause reason, if paused.
    pub fn pause_reason(&self) -> Option<PauseReason> {
        self.0.pause_reason.lock().unwrap().clone()
    }

    /// Total number of times the bridge has been paused.
    pub fn total_pauses(&self) -> u64 {
        self.0.total_pauses.load(RLX)
    }

    // ── Pause / Resume ──────────────────────────────────────────────────────

    /// Pause the bridge with a given reason.
    fn pause(&self, reason: PauseReason, now: u64) {
        self.0.paused.store(true, RLX);
        self.0.paused_at.store(now, RLX);
        self.0.total_pauses.fetch_add(1, RLX);
        *self.0.pause_reason.lock().unwrap() = Some(reason);
    }

    /// Resume the bridge (only if the current pause is auto-recoverable or forced).
    pub fn resume(&self, force: bool) -> Result<(), CircuitBreakerError> {
        if !self.is_paused() {
            return Err(CircuitBreakerError::NotPaused);
        }
        let reason = self.0.pause_reason.lock().unwrap().clone();
        if let Some(ref r) = reason {
            if !r.auto_recoverable() && !force {
                return Err(CircuitBreakerError::ManualResumeRequired {
                    reason: format!("{:?}", r),
                });
            }
        }
        self.0.paused.store(false, RLX);
        self.0.consecutive_proof_failures.store(0, RLX);
        self.0.consecutive_settlement_failures.store(0, RLX);
        *self.0.pause_reason.lock().unwrap() = None;
        Ok(())
    }

    /// Guardian pause: any authorized guardian can halt the bridge.
    pub fn guardian_pause(
        &self,
        guardian_key_hash: &[u8; 32],
        message: impl Into<String>,
        now: u64,
    ) -> Result<(), CircuitBreakerError> {
        let guardians = self.0.guardians.lock().unwrap();
        if !guardians.iter().any(|g| g == guardian_key_hash) {
            return Err(CircuitBreakerError::UnauthorizedGuardian);
        }
        drop(guardians);
        let guardian_hex: String = guardian_key_hash
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        self.pause(
            PauseReason::GuardianPause {
                guardian: guardian_hex,
                message: message.into(),
            },
            now,
        );
        Ok(())
    }

    /// Register an authorized guardian key hash.
    pub fn add_guardian(&self, key_hash: [u8; 32]) {
        self.0.guardians.lock().unwrap().push(key_hash);
    }

    // ── Event Recording ─────────────────────────────────────────────────────

    /// Record a proof generation result. Triggers auto-pause on consecutive failures.
    pub fn record_proof_result(&self, success: bool, now: u64) {
        if success {
            self.0.consecutive_proof_failures.store(0, RLX);
        } else {
            let failures = self.0.consecutive_proof_failures.fetch_add(1, RLX) + 1;
            if failures >= PROOF_FAILURE_THRESHOLD as u64 {
                self.pause(
                    PauseReason::ProofFailures {
                        count: failures as u32,
                    },
                    now,
                );
            }
        }
    }

    /// Record a settlement result. Triggers auto-pause on consecutive failures.
    pub fn record_settlement_result(&self, success: bool, now: u64) {
        if success {
            self.0.consecutive_settlement_failures.store(0, RLX);
        } else {
            let failures = self.0.consecutive_settlement_failures.fetch_add(1, RLX) + 1;
            if failures >= SETTLEMENT_FAILURE_THRESHOLD as u64 {
                self.pause(
                    PauseReason::SettlementFailures {
                        count: failures as u32,
                    },
                    now,
                );
            }
        }
    }

    /// Record an outflow (transfer leaving the bridge). Triggers TVL drain guard
    /// if cumulative outflow in the window exceeds the limit.
    pub fn record_outflow(&self, amount_cents: u64, now: u64) {
        let mut outflows = self.0.outflows.lock().unwrap();
        // Evict stale entries outside the window
        while outflows
            .front()
            .is_some_and(|r| now - r.timestamp > DRAIN_WINDOW_SECS)
        {
            outflows.pop_front();
        }
        outflows.push_back(OutflowRecord {
            amount_cents,
            timestamp: now,
        });

        let total: u64 = outflows.iter().map(|r| r.amount_cents).sum();
        if total > TVL_DRAIN_LIMIT_CENTS {
            drop(outflows);
            self.pause(
                PauseReason::TvlDrain {
                    outflow_cents: total,
                    window_secs: DRAIN_WINDOW_SECS,
                },
                now,
            );
        }
    }

    // ── Auto-recovery check ─────────────────────────────────────────────────

    /// Check if the cooldown period has elapsed and the pause is auto-recoverable.
    /// Returns `true` if the bridge was resumed.
    pub fn check_auto_recovery(&self, now: u64) -> bool {
        if !self.is_paused() {
            return false;
        }
        let paused_at = self.0.paused_at.load(RLX);
        let elapsed = now.saturating_sub(paused_at);
        if elapsed < COOLDOWN_SECS {
            return false;
        }
        let reason = self.0.pause_reason.lock().unwrap().clone();
        if let Some(r) = reason {
            if r.auto_recoverable() {
                self.0.paused.store(false, RLX);
                self.0.consecutive_proof_failures.store(0, RLX);
                self.0.consecutive_settlement_failures.store(0, RLX);
                *self.0.pause_reason.lock().unwrap() = None;
                return true;
            }
        }
        false
    }

    // ── Status snapshot ─────────────────────────────────────────────────────

    pub fn status_json(&self) -> serde_json::Value {
        let reason = self.0.pause_reason.lock().unwrap().clone();
        serde_json::json!({
            "operational": self.is_operational(),
            "paused": self.is_paused(),
            "pause_reason": reason.as_ref().map(|r| format!("{:?}", r)),
            "severity": reason.as_ref().map(|r| r.severity()),
            "auto_recoverable": reason.as_ref().map(|r| r.auto_recoverable()),
            "total_pauses": self.total_pauses(),
            "consecutive_proof_failures": self.0.consecutive_proof_failures.load(RLX),
            "consecutive_settlement_failures": self.0.consecutive_settlement_failures.load(RLX),
        })
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum CircuitBreakerError {
    NotPaused,
    ManualResumeRequired { reason: String },
    UnauthorizedGuardian,
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_starts_operational() {
        let cb = CircuitBreaker::new();
        assert!(cb.is_operational());
        assert!(!cb.is_paused());
        assert!(cb.pause_reason().is_none());
    }

    #[test]
    fn test_proof_failures_trigger_pause() {
        let cb = CircuitBreaker::new();
        for i in 0..PROOF_FAILURE_THRESHOLD {
            cb.record_proof_result(false, i as u64);
        }
        assert!(cb.is_paused());
        assert!(matches!(
            cb.pause_reason().unwrap(),
            PauseReason::ProofFailures { count: 5 }
        ));
    }

    #[test]
    fn test_success_resets_failure_count() {
        let cb = CircuitBreaker::new();
        for _ in 0..(PROOF_FAILURE_THRESHOLD - 1) {
            cb.record_proof_result(false, 0);
        }
        assert!(cb.is_operational()); // not yet at threshold
        cb.record_proof_result(true, 0); // resets
                                         // Now need another full run of failures to trigger
        for _ in 0..(PROOF_FAILURE_THRESHOLD - 1) {
            cb.record_proof_result(false, 0);
        }
        assert!(cb.is_operational()); // still not paused
    }

    #[test]
    fn test_settlement_failures_trigger_pause() {
        let cb = CircuitBreaker::new();
        for i in 0..SETTLEMENT_FAILURE_THRESHOLD {
            cb.record_settlement_result(false, i as u64);
        }
        assert!(cb.is_paused());
        assert!(matches!(
            cb.pause_reason().unwrap(),
            PauseReason::SettlementFailures { count: 3 }
        ));
    }

    #[test]
    fn test_auto_recovery_after_cooldown() {
        let cb = CircuitBreaker::new();
        // Trigger via proof failures (auto-recoverable)
        for i in 0..PROOF_FAILURE_THRESHOLD {
            cb.record_proof_result(false, i as u64);
        }
        assert!(cb.is_paused());

        // Before cooldown — no recovery
        assert!(!cb.check_auto_recovery(100));
        assert!(cb.is_paused());

        // After cooldown
        let recover_time = PROOF_FAILURE_THRESHOLD as u64 + COOLDOWN_SECS + 1;
        assert!(cb.check_auto_recovery(recover_time));
        assert!(cb.is_operational());
    }

    #[test]
    fn test_guardian_pause_not_auto_recoverable() {
        let cb = CircuitBreaker::new();
        let guardian = [0xAA; 32];
        cb.add_guardian(guardian);
        cb.guardian_pause(&guardian, "security incident", 100)
            .unwrap();
        assert!(cb.is_paused());

        // Auto-recovery should NOT work for guardian pauses
        assert!(!cb.check_auto_recovery(100 + COOLDOWN_SECS + 1));
        assert!(cb.is_paused());

        // Force resume works
        cb.resume(true).unwrap();
        assert!(cb.is_operational());
    }

    #[test]
    fn test_unauthorized_guardian_rejected() {
        let cb = CircuitBreaker::new();
        let unknown = [0xBB; 32];
        let err = cb.guardian_pause(&unknown, "hack", 0).unwrap_err();
        assert_eq!(err, CircuitBreakerError::UnauthorizedGuardian);
        assert!(cb.is_operational());
    }

    #[test]
    fn test_tvl_drain_guard() {
        let cb = CircuitBreaker::new();
        let now = 1000u64;
        // Single large outflow exceeding $1M
        cb.record_outflow(TVL_DRAIN_LIMIT_CENTS + 1, now);
        assert!(cb.is_paused());
        assert!(matches!(
            cb.pause_reason().unwrap(),
            PauseReason::TvlDrain { .. }
        ));
    }

    #[test]
    fn test_tvl_drain_cumulative_within_window() {
        let cb = CircuitBreaker::new();
        let chunk = TVL_DRAIN_LIMIT_CENTS / 3;
        // 4 chunks within the window = exceeds limit
        for i in 0..4 {
            cb.record_outflow(chunk, i * 10);
        }
        assert!(cb.is_paused());
    }

    #[test]
    fn test_tvl_drain_stale_entries_evicted() {
        let cb = CircuitBreaker::new();
        let chunk = TVL_DRAIN_LIMIT_CENTS / 2;
        // First outflow at time 0
        cb.record_outflow(chunk, 0);
        // Second outflow AFTER the window (time > DRAIN_WINDOW_SECS)
        cb.record_outflow(chunk, DRAIN_WINDOW_SECS + 10);
        // Should NOT be paused — first entry evicted
        assert!(cb.is_operational());
    }

    #[test]
    fn test_resume_not_paused_error() {
        let cb = CircuitBreaker::new();
        let err = cb.resume(false).unwrap_err();
        assert_eq!(err, CircuitBreakerError::NotPaused);
    }

    #[test]
    fn test_manual_resume_required_for_tvl_drain() {
        let cb = CircuitBreaker::new();
        cb.record_outflow(TVL_DRAIN_LIMIT_CENTS + 1, 0);
        assert!(cb.is_paused());

        let err = cb.resume(false).unwrap_err();
        assert!(matches!(
            err,
            CircuitBreakerError::ManualResumeRequired { .. }
        ));

        cb.resume(true).unwrap();
        assert!(cb.is_operational());
    }

    #[test]
    fn test_status_json_structure() {
        let cb = CircuitBreaker::new();
        let j = cb.status_json();
        assert_eq!(j["operational"], true);
        assert_eq!(j["paused"], false);
        assert!(j["pause_reason"].is_null());
    }

    #[test]
    fn test_total_pauses_counter() {
        let cb = CircuitBreaker::new();
        // Trigger and recover multiple times
        for _ in 0..PROOF_FAILURE_THRESHOLD {
            cb.record_proof_result(false, 0);
        }
        cb.resume(true).unwrap();
        for _ in 0..SETTLEMENT_FAILURE_THRESHOLD {
            cb.record_settlement_result(false, 100);
        }
        cb.resume(true).unwrap();
        assert_eq!(cb.total_pauses(), 2);
    }
}
