/// Atomic cross-chain settlement for InterLink (Phase 8)
///
/// Two-phase commit protocol for cross-chain transfers with
/// timeout-based rollback and escrow state machine.
///
/// Protocol:
///   Phase 1 (PREPARE):
///     - Source chain: lock funds in escrow
///     - Generate ZK proof of lock
///     - Destination chain: prepare mint/release
///
///   Phase 2 (COMMIT or ROLLBACK):
///     - COMMIT: destination confirms receipt → source finalizes lock
///     - ROLLBACK: timeout expires → source unlocks funds to sender
///
/// Guarantees:
///   - Funds never lost: either fully transferred or fully returned
///   - No double-spend: escrow prevents spending locked funds
///   - Timeout safety: stuck transfers auto-rollback after deadline
///
/// Comparison:
///   Wormhole:  no atomic guarantee — funds can be stuck if guardian fails
///   Across:    optimistic — relayer fronts, repaid later (can dispute)
///   HTLC:      hash time-locked contracts (Bitcoin Lightning style)
///   InterLink: ZK-verified two-phase commit with automatic rollback
use std::collections::HashMap;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Default escrow timeout (30 minutes).
pub const DEFAULT_ESCROW_TIMEOUT_SECS: u64 = 30 * 60;
/// Maximum escrow timeout (24 hours).
pub const MAX_ESCROW_TIMEOUT_SECS: u64 = 24 * 3600;
/// Minimum escrow timeout (5 minutes).
pub const MIN_ESCROW_TIMEOUT_SECS: u64 = 5 * 60;
/// Grace period after timeout before forceful rollback (2 minutes).
pub const ROLLBACK_GRACE_SECS: u64 = 120;

// ─── Types ──────────────────────────────────────────────────────────────────

/// State of an atomic settlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementState {
    /// Funds locked in escrow on source chain.
    Prepared,
    /// ZK proof generated for the lock.
    ProofReady,
    /// Destination chain confirmed receipt.
    Committed,
    /// Settlement complete on both chains.
    Finalized,
    /// Timeout expired, funds returned to sender.
    RolledBack,
    /// Settlement failed (with reason).
    Failed { reason: String },
}

/// An atomic settlement record.
#[derive(Debug, Clone)]
pub struct AtomicSettlement {
    /// Unique settlement ID.
    pub settlement_id: String,
    /// Source chain ID.
    pub source_chain: u32,
    /// Destination chain ID.
    pub dest_chain: u32,
    /// Sender address on source.
    pub sender: String,
    /// Receiver address on destination.
    pub receiver: String,
    /// Amount locked in escrow.
    pub amount: u128,
    /// Token identifier.
    pub token: String,
    /// Current state.
    pub state: SettlementState,
    /// Timestamp when escrow was created.
    pub created_at: u64,
    /// Escrow deadline (auto-rollback after this).
    pub deadline: u64,
    /// ZK proof commitment (set in ProofReady state).
    pub proof_commitment: Option<String>,
    /// Source chain tx hash (lock tx).
    pub source_tx: Option<String>,
    /// Destination chain tx hash (mint/release tx).
    pub dest_tx: Option<String>,
    /// Timestamp of last state change.
    pub updated_at: u64,
}

/// Settlement creation parameters.
#[derive(Debug, Clone)]
pub struct SettlementParams {
    pub source_chain: u32,
    pub dest_chain: u32,
    pub sender: String,
    pub receiver: String,
    pub amount: u128,
    pub token: String,
    pub timeout_secs: u64,
}

// ─── Settlement Engine ──────────────────────────────────────────────────────

pub struct AtomicSettlementEngine {
    /// All settlements by ID.
    settlements: HashMap<String, AtomicSettlement>,
    /// Index: sender → [settlement_ids].
    by_sender: HashMap<String, Vec<String>>,
    /// Counter for ID generation.
    next_id: u64,
    /// Statistics.
    total_committed: u64,
    total_rolled_back: u64,
    total_failed: u64,
}

impl AtomicSettlementEngine {
    pub fn new() -> Self {
        AtomicSettlementEngine {
            settlements: HashMap::new(),
            by_sender: HashMap::new(),
            next_id: 0,
            total_committed: 0,
            total_rolled_back: 0,
            total_failed: 0,
        }
    }

    /// Phase 1: Create escrow (PREPARE).
    pub fn prepare(&mut self, params: SettlementParams, now: u64) -> Result<String, AtomicError> {
        // Validate timeout
        let timeout = params
            .timeout_secs
            .clamp(MIN_ESCROW_TIMEOUT_SECS, MAX_ESCROW_TIMEOUT_SECS);

        if params.amount == 0 {
            return Err(AtomicError::ZeroAmount);
        }
        if params.source_chain == params.dest_chain {
            return Err(AtomicError::SameChain);
        }

        let settlement_id = format!("atomic_{}", self.next_id);
        self.next_id += 1;

        let settlement = AtomicSettlement {
            settlement_id: settlement_id.clone(),
            source_chain: params.source_chain,
            dest_chain: params.dest_chain,
            sender: params.sender.clone(),
            receiver: params.receiver,
            amount: params.amount,
            token: params.token,
            state: SettlementState::Prepared,
            created_at: now,
            deadline: now + timeout,
            proof_commitment: None,
            source_tx: None,
            dest_tx: None,
            updated_at: now,
        };

        self.by_sender
            .entry(params.sender)
            .or_default()
            .push(settlement_id.clone());
        self.settlements.insert(settlement_id.clone(), settlement);

        Ok(settlement_id)
    }

    /// Attach ZK proof to a prepared settlement.
    pub fn attach_proof(
        &mut self,
        settlement_id: &str,
        proof_commitment: impl Into<String>,
        source_tx: impl Into<String>,
        now: u64,
    ) -> Result<(), AtomicError> {
        let s = self
            .settlements
            .get_mut(settlement_id)
            .ok_or(AtomicError::NotFound)?;
        if s.state != SettlementState::Prepared {
            return Err(AtomicError::InvalidTransition {
                from: format!("{:?}", s.state),
                to: "ProofReady".into(),
            });
        }
        if now > s.deadline {
            s.state = SettlementState::RolledBack;
            s.updated_at = now;
            self.total_rolled_back += 1;
            return Err(AtomicError::Expired);
        }
        s.proof_commitment = Some(proof_commitment.into());
        s.source_tx = Some(source_tx.into());
        s.state = SettlementState::ProofReady;
        s.updated_at = now;
        Ok(())
    }

    /// Phase 2a: COMMIT — destination confirms receipt.
    pub fn commit(
        &mut self,
        settlement_id: &str,
        dest_tx: impl Into<String>,
        now: u64,
    ) -> Result<(), AtomicError> {
        let s = self
            .settlements
            .get_mut(settlement_id)
            .ok_or(AtomicError::NotFound)?;
        if s.state != SettlementState::ProofReady {
            return Err(AtomicError::InvalidTransition {
                from: format!("{:?}", s.state),
                to: "Committed".into(),
            });
        }
        if now > s.deadline {
            s.state = SettlementState::RolledBack;
            s.updated_at = now;
            self.total_rolled_back += 1;
            return Err(AtomicError::Expired);
        }
        s.dest_tx = Some(dest_tx.into());
        s.state = SettlementState::Committed;
        s.updated_at = now;
        self.total_committed += 1;
        Ok(())
    }

    /// Finalize a committed settlement (both chains confirmed).
    pub fn finalize(&mut self, settlement_id: &str, now: u64) -> Result<(), AtomicError> {
        let s = self
            .settlements
            .get_mut(settlement_id)
            .ok_or(AtomicError::NotFound)?;
        if s.state != SettlementState::Committed {
            return Err(AtomicError::InvalidTransition {
                from: format!("{:?}", s.state),
                to: "Finalized".into(),
            });
        }
        s.state = SettlementState::Finalized;
        s.updated_at = now;
        Ok(())
    }

    /// Phase 2b: ROLLBACK — timeout expired or explicit rollback.
    pub fn rollback(
        &mut self,
        settlement_id: &str,
        reason: impl Into<String>,
        now: u64,
    ) -> Result<(), AtomicError> {
        let s = self
            .settlements
            .get_mut(settlement_id)
            .ok_or(AtomicError::NotFound)?;
        match &s.state {
            SettlementState::Prepared | SettlementState::ProofReady => {
                s.state = SettlementState::RolledBack;
                s.updated_at = now;
                self.total_rolled_back += 1;
                Ok(())
            }
            SettlementState::Committed | SettlementState::Finalized => {
                Err(AtomicError::CannotRollbackCommitted)
            }
            _ => {
                let _ = reason.into();
                Err(AtomicError::InvalidTransition {
                    from: format!("{:?}", s.state),
                    to: "RolledBack".into(),
                })
            }
        }
    }

    /// Process all timed-out settlements. Returns IDs of rolled-back settlements.
    pub fn process_timeouts(&mut self, now: u64) -> Vec<String> {
        let mut rolled_back = Vec::new();
        let expired_ids: Vec<String> = self
            .settlements
            .iter()
            .filter(|(_, s)| {
                matches!(
                    s.state,
                    SettlementState::Prepared | SettlementState::ProofReady
                ) && now > s.deadline + ROLLBACK_GRACE_SECS
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in expired_ids {
            if let Some(s) = self.settlements.get_mut(&id) {
                s.state = SettlementState::RolledBack;
                s.updated_at = now;
                self.total_rolled_back += 1;
                rolled_back.push(id);
            }
        }
        rolled_back
    }

    /// Get settlement by ID.
    pub fn get(&self, settlement_id: &str) -> Option<&AtomicSettlement> {
        self.settlements.get(settlement_id)
    }

    /// Get all settlements by sender.
    pub fn get_by_sender(&self, sender: &str) -> Vec<&AtomicSettlement> {
        self.by_sender
            .get(sender)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.settlements.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Count of active (non-terminal) settlements.
    pub fn active_count(&self) -> usize {
        self.settlements
            .values()
            .filter(|s| {
                matches!(
                    s.state,
                    SettlementState::Prepared
                        | SettlementState::ProofReady
                        | SettlementState::Committed
                )
            })
            .count()
    }

    /// Stats as JSON.
    pub fn stats_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total_settlements": self.settlements.len(),
            "active": self.active_count(),
            "committed": self.total_committed,
            "rolled_back": self.total_rolled_back,
            "failed": self.total_failed,
        })
    }
}

impl Default for AtomicSettlementEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum AtomicError {
    NotFound,
    ZeroAmount,
    SameChain,
    Expired,
    InvalidTransition { from: String, to: String },
    CannotRollbackCommitted,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_params() -> SettlementParams {
        SettlementParams {
            source_chain: 1,
            dest_chain: 900,
            sender: "0xAlice".into(),
            receiver: "SolBob".into(),
            amount: 1_000_000_000_000_000_000,
            token: "native".into(),
            timeout_secs: DEFAULT_ESCROW_TIMEOUT_SECS,
        }
    }

    #[test]
    fn test_prepare_creates_escrow() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();
        let s = engine.get(&id).unwrap();
        assert_eq!(s.state, SettlementState::Prepared);
        assert_eq!(s.deadline, 1000 + DEFAULT_ESCROW_TIMEOUT_SECS);
    }

    #[test]
    fn test_zero_amount_rejected() {
        let mut engine = AtomicSettlementEngine::new();
        let mut params = sample_params();
        params.amount = 0;
        assert_eq!(engine.prepare(params, 1000), Err(AtomicError::ZeroAmount));
    }

    #[test]
    fn test_same_chain_rejected() {
        let mut engine = AtomicSettlementEngine::new();
        let mut params = sample_params();
        params.dest_chain = params.source_chain;
        assert_eq!(engine.prepare(params, 1000), Err(AtomicError::SameChain));
    }

    #[test]
    fn test_full_happy_path() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();

        engine
            .attach_proof(&id, "proof_abc", "0xSourceTx", 1010)
            .unwrap();
        assert_eq!(engine.get(&id).unwrap().state, SettlementState::ProofReady);

        engine.commit(&id, "SolDestTx", 1020).unwrap();
        assert_eq!(engine.get(&id).unwrap().state, SettlementState::Committed);

        engine.finalize(&id, 1030).unwrap();
        assert_eq!(engine.get(&id).unwrap().state, SettlementState::Finalized);
    }

    #[test]
    fn test_rollback_from_prepared() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();
        engine.rollback(&id, "user cancelled", 1100).unwrap();
        assert_eq!(engine.get(&id).unwrap().state, SettlementState::RolledBack);
    }

    #[test]
    fn test_rollback_from_proof_ready() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();
        engine.attach_proof(&id, "proof", "tx", 1010).unwrap();
        engine.rollback(&id, "dest chain down", 1100).unwrap();
        assert_eq!(engine.get(&id).unwrap().state, SettlementState::RolledBack);
    }

    #[test]
    fn test_cannot_rollback_committed() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();
        engine.attach_proof(&id, "p", "t", 1010).unwrap();
        engine.commit(&id, "dt", 1020).unwrap();
        assert_eq!(
            engine.rollback(&id, "too late", 1100),
            Err(AtomicError::CannotRollbackCommitted)
        );
    }

    #[test]
    fn test_attach_proof_after_deadline_auto_rollback() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();
        let deadline = 1000 + DEFAULT_ESCROW_TIMEOUT_SECS;
        let result = engine.attach_proof(&id, "p", "t", deadline + 1);
        assert_eq!(result, Err(AtomicError::Expired));
        assert_eq!(engine.get(&id).unwrap().state, SettlementState::RolledBack);
    }

    #[test]
    fn test_commit_after_deadline_auto_rollback() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();
        engine.attach_proof(&id, "p", "t", 1010).unwrap();
        let deadline = 1000 + DEFAULT_ESCROW_TIMEOUT_SECS;
        let result = engine.commit(&id, "dt", deadline + 1);
        assert_eq!(result, Err(AtomicError::Expired));
        assert_eq!(engine.get(&id).unwrap().state, SettlementState::RolledBack);
    }

    #[test]
    fn test_process_timeouts() {
        let mut engine = AtomicSettlementEngine::new();
        let id1 = engine.prepare(sample_params(), 1000).unwrap();
        let mut params2 = sample_params();
        params2.sender = "0xBob".into();
        let id2 = engine.prepare(params2, 2000).unwrap();

        // Only id1 should expire
        let deadline1 = 1000 + DEFAULT_ESCROW_TIMEOUT_SECS + ROLLBACK_GRACE_SECS + 1;
        let rolled = engine.process_timeouts(deadline1);
        assert_eq!(rolled, vec![id1]);
        assert_eq!(engine.get(&id2).unwrap().state, SettlementState::Prepared);
    }

    #[test]
    fn test_invalid_state_transitions() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();
        // Can't commit without proof
        assert!(matches!(
            engine.commit(&id, "dt", 1010),
            Err(AtomicError::InvalidTransition { .. })
        ));
        // Can't finalize without commit
        assert!(matches!(
            engine.finalize(&id, 1010),
            Err(AtomicError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn test_timeout_clamped() {
        let mut engine = AtomicSettlementEngine::new();
        let mut params = sample_params();
        params.timeout_secs = 1; // below minimum
        let id = engine.prepare(params, 1000).unwrap();
        let s = engine.get(&id).unwrap();
        assert_eq!(s.deadline, 1000 + MIN_ESCROW_TIMEOUT_SECS);
    }

    #[test]
    fn test_get_by_sender() {
        let mut engine = AtomicSettlementEngine::new();
        engine.prepare(sample_params(), 1000).unwrap();
        engine.prepare(sample_params(), 1001).unwrap();
        assert_eq!(engine.get_by_sender("0xAlice").len(), 2);
        assert_eq!(engine.get_by_sender("0xBob").len(), 0);
    }

    #[test]
    fn test_active_count() {
        let mut engine = AtomicSettlementEngine::new();
        engine.prepare(sample_params(), 1000).unwrap();
        let id2 = engine.prepare(sample_params(), 1001).unwrap();
        assert_eq!(engine.active_count(), 2);
        engine.rollback(&id2, "cancel", 1100).unwrap();
        assert_eq!(engine.active_count(), 1);
    }

    #[test]
    fn test_stats_json() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();
        engine.attach_proof(&id, "p", "t", 1010).unwrap();
        engine.commit(&id, "dt", 1020).unwrap();
        let j = engine.stats_json();
        assert_eq!(j["committed"], 1);
        assert_eq!(j["total_settlements"], 1);
    }

    #[test]
    fn test_proof_commitment_stored() {
        let mut engine = AtomicSettlementEngine::new();
        let id = engine.prepare(sample_params(), 1000).unwrap();
        engine
            .attach_proof(&id, "0xProofHash", "0xLockTx", 1010)
            .unwrap();
        let s = engine.get(&id).unwrap();
        assert_eq!(s.proof_commitment.as_deref(), Some("0xProofHash"));
        assert_eq!(s.source_tx.as_deref(), Some("0xLockTx"));
    }
}
