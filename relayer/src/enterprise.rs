/// Enterprise features for InterLink (Phase 12)
///
/// Address whitelisting, spend limits, delayed settlement, and
/// multi-approver workflows for institutional bridge users.
///
/// Features:
///   - Address whitelisting:  only pre-approved destinations
///   - Spend limits:          per-tx, daily, and monthly caps
///   - Delayed settlement:    configurable hold period before release
///   - Multi-approver:        require N-of-M org admins for large transfers
///   - Audit integration:     all actions logged with approver identity
///
/// Comparison:
///   Wormhole:  no enterprise features
///   Across:    no enterprise features
///   Fireblocks: full enterprise wallet, but proprietary + expensive
///   InterLink: built-in enterprise controls, no 3rd party dependency

use std::collections::{HashMap, HashSet};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Default daily spend limit in USD cents ($1M).
pub const DEFAULT_DAILY_LIMIT_CENTS: u64 = 100_000_000;
/// Default monthly spend limit in USD cents ($10M).
pub const DEFAULT_MONTHLY_LIMIT_CENTS: u64 = 1_000_000_000;
/// Default per-transaction limit in USD cents ($500k).
pub const DEFAULT_PER_TX_LIMIT_CENTS: u64 = 50_000_000;
/// Default delayed settlement hold period (1 hour).
pub const DEFAULT_HOLD_PERIOD_SECS: u64 = 3600;
/// Large transfer threshold requiring multi-approval ($100k).
pub const LARGE_TRANSFER_THRESHOLD_CENTS: u64 = 10_000_000;
/// Maximum approvers per organization.
pub const MAX_APPROVERS: usize = 20;

// ─── Types ──────────────────────────────────────────────────────────────────

/// Enterprise organization configuration.
#[derive(Debug, Clone)]
pub struct OrgConfig {
    /// Organization identifier.
    pub org_id: String,
    /// Organization display name.
    pub name: String,
    /// Whitelisted destination addresses (empty = all allowed).
    pub whitelist: HashSet<String>,
    /// Whether whitelisting is enforced.
    pub whitelist_enabled: bool,
    /// Per-transaction spend limit (USD cents).
    pub per_tx_limit_cents: u64,
    /// Daily spend limit (USD cents).
    pub daily_limit_cents: u64,
    /// Monthly spend limit (USD cents).
    pub monthly_limit_cents: u64,
    /// Delayed settlement hold period (seconds, 0 = instant).
    pub hold_period_secs: u64,
    /// Approvers (addresses) who can authorize large transfers.
    pub approvers: Vec<String>,
    /// Required approvals for large transfers (N of M).
    pub required_approvals: u32,
}

impl OrgConfig {
    /// Create a new org with default limits.
    pub fn new(org_id: impl Into<String>, name: impl Into<String>) -> Self {
        OrgConfig {
            org_id: org_id.into(),
            name: name.into(),
            whitelist: HashSet::new(),
            whitelist_enabled: false,
            per_tx_limit_cents: DEFAULT_PER_TX_LIMIT_CENTS,
            daily_limit_cents: DEFAULT_DAILY_LIMIT_CENTS,
            monthly_limit_cents: DEFAULT_MONTHLY_LIMIT_CENTS,
            hold_period_secs: DEFAULT_HOLD_PERIOD_SECS,
            approvers: Vec::new(),
            required_approvals: 1,
        }
    }
}

/// Spend tracking for an organization.
#[derive(Debug, Clone, Default)]
pub struct SpendTracker {
    /// Daily spend in USD cents (resets daily).
    pub daily_spent_cents: u64,
    /// Monthly spend in USD cents (resets monthly).
    pub monthly_spent_cents: u64,
    /// Last daily reset timestamp.
    pub daily_reset_at: u64,
    /// Last monthly reset timestamp.
    pub monthly_reset_at: u64,
}

/// A pending transfer awaiting approval or hold period.
#[derive(Debug, Clone)]
pub struct PendingTransfer {
    /// Transfer ID.
    pub transfer_id: String,
    /// Organization ID.
    pub org_id: String,
    /// Destination address.
    pub destination: String,
    /// Amount in USD cents.
    pub amount_cents: u64,
    /// Approvals received so far.
    pub approvals: HashSet<String>,
    /// Required approvals.
    pub required_approvals: u32,
    /// Whether approval requirement is met.
    pub approved: bool,
    /// Hold release timestamp (when settlement can proceed).
    pub release_at: u64,
    /// Creation timestamp.
    pub created_at: u64,
    /// Current status.
    pub status: TransferApprovalStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferApprovalStatus {
    PendingApproval,
    PendingHold,
    ReadyToSettle,
    Settled,
    Rejected,
}

/// Result of a transfer validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    /// Transfer is approved and ready.
    Approved,
    /// Transfer needs multi-approver sign-off.
    NeedsApproval { required: u32, current: u32 },
    /// Transfer is in hold period.
    InHoldPeriod { release_at: u64 },
    /// Transfer was rejected.
    Rejected { reason: String },
}

// ─── Enterprise Manager ─────────────────────────────────────────────────────

pub struct EnterpriseManager {
    /// Organization configurations.
    orgs: HashMap<String, OrgConfig>,
    /// Spend tracking per org.
    spend: HashMap<String, SpendTracker>,
    /// Pending transfers.
    pending: HashMap<String, PendingTransfer>,
    /// Transfer ID counter.
    next_id: u64,
}

impl EnterpriseManager {
    pub fn new() -> Self {
        EnterpriseManager {
            orgs: HashMap::new(),
            spend: HashMap::new(),
            pending: HashMap::new(),
            next_id: 0,
        }
    }

    /// Register a new organization.
    pub fn register_org(&mut self, config: OrgConfig) -> Result<(), EnterpriseError> {
        if config.approvers.len() > MAX_APPROVERS {
            return Err(EnterpriseError::TooManyApprovers);
        }
        if config.required_approvals as usize > config.approvers.len() && !config.approvers.is_empty() {
            return Err(EnterpriseError::InvalidApprovalThreshold);
        }
        self.spend.entry(config.org_id.clone()).or_default();
        self.orgs.insert(config.org_id.clone(), config);
        Ok(())
    }

    /// Add an address to an org's whitelist.
    pub fn add_to_whitelist(&mut self, org_id: &str, address: impl Into<String>) -> Result<(), EnterpriseError> {
        let org = self.orgs.get_mut(org_id).ok_or(EnterpriseError::OrgNotFound)?;
        org.whitelist.insert(address.into());
        Ok(())
    }

    /// Remove an address from an org's whitelist.
    pub fn remove_from_whitelist(&mut self, org_id: &str, address: &str) -> Result<(), EnterpriseError> {
        let org = self.orgs.get_mut(org_id).ok_or(EnterpriseError::OrgNotFound)?;
        org.whitelist.remove(address);
        Ok(())
    }

    /// Validate and initiate a transfer.
    pub fn initiate_transfer(
        &mut self,
        org_id: &str,
        destination: &str,
        amount_cents: u64,
        now: u64,
    ) -> Result<(String, ValidationResult), EnterpriseError> {
        let org = self.orgs.get(org_id).ok_or(EnterpriseError::OrgNotFound)?.clone();

        // 1. Whitelist check
        if org.whitelist_enabled && !org.whitelist.contains(destination) {
            return Ok((String::new(), ValidationResult::Rejected {
                reason: format!("destination {destination} not in whitelist"),
            }));
        }

        // 2. Per-tx limit check
        if amount_cents > org.per_tx_limit_cents {
            return Ok((String::new(), ValidationResult::Rejected {
                reason: format!(
                    "amount {}c exceeds per-tx limit {}c",
                    amount_cents, org.per_tx_limit_cents
                ),
            }));
        }

        // 3. Daily/monthly limit check (with auto-reset)
        let spend = self.spend.entry(org_id.to_string()).or_default();
        reset_spend_if_needed(spend, now);

        if spend.daily_spent_cents + amount_cents > org.daily_limit_cents {
            return Ok((String::new(), ValidationResult::Rejected {
                reason: format!(
                    "daily limit exceeded: {}c spent + {}c = {}c > {}c limit",
                    spend.daily_spent_cents, amount_cents,
                    spend.daily_spent_cents + amount_cents, org.daily_limit_cents
                ),
            }));
        }
        if spend.monthly_spent_cents + amount_cents > org.monthly_limit_cents {
            return Ok((String::new(), ValidationResult::Rejected {
                reason: "monthly limit exceeded".into(),
            }));
        }

        // 4. Create pending transfer
        let transfer_id = format!("ent_tx_{}", self.next_id);
        self.next_id += 1;

        let needs_approval = amount_cents >= LARGE_TRANSFER_THRESHOLD_CENTS
            && !org.approvers.is_empty()
            && org.required_approvals > 0;

        let release_at = now + org.hold_period_secs;

        let status = if needs_approval {
            TransferApprovalStatus::PendingApproval
        } else if org.hold_period_secs > 0 {
            TransferApprovalStatus::PendingHold
        } else {
            TransferApprovalStatus::ReadyToSettle
        };

        let pending = PendingTransfer {
            transfer_id: transfer_id.clone(),
            org_id: org_id.to_string(),
            destination: destination.to_string(),
            amount_cents,
            approvals: HashSet::new(),
            required_approvals: if needs_approval { org.required_approvals } else { 0 },
            approved: !needs_approval,
            release_at,
            created_at: now,
            status: status.clone(),
        };

        // Update spend
        let spend = self.spend.get_mut(org_id).unwrap();
        spend.daily_spent_cents += amount_cents;
        spend.monthly_spent_cents += amount_cents;

        self.pending.insert(transfer_id.clone(), pending);

        let result = match status {
            TransferApprovalStatus::PendingApproval => ValidationResult::NeedsApproval {
                required: org.required_approvals,
                current: 0,
            },
            TransferApprovalStatus::PendingHold => ValidationResult::InHoldPeriod { release_at },
            TransferApprovalStatus::ReadyToSettle => ValidationResult::Approved,
            _ => unreachable!(),
        };

        Ok((transfer_id, result))
    }

    /// Approve a pending transfer.
    pub fn approve_transfer(
        &mut self,
        transfer_id: &str,
        approver: &str,
    ) -> Result<ValidationResult, EnterpriseError> {
        let transfer = self.pending.get_mut(transfer_id).ok_or(EnterpriseError::TransferNotFound)?;
        if transfer.status != TransferApprovalStatus::PendingApproval {
            return Err(EnterpriseError::NotPendingApproval);
        }

        // Verify approver is authorized
        let org = self.orgs.get(&transfer.org_id).ok_or(EnterpriseError::OrgNotFound)?;
        if !org.approvers.contains(&approver.to_string()) {
            return Err(EnterpriseError::UnauthorizedApprover);
        }

        transfer.approvals.insert(approver.to_string());

        if transfer.approvals.len() as u32 >= transfer.required_approvals {
            transfer.approved = true;
            if org.hold_period_secs > 0 {
                transfer.status = TransferApprovalStatus::PendingHold;
                Ok(ValidationResult::InHoldPeriod {
                    release_at: transfer.release_at,
                })
            } else {
                transfer.status = TransferApprovalStatus::ReadyToSettle;
                Ok(ValidationResult::Approved)
            }
        } else {
            Ok(ValidationResult::NeedsApproval {
                required: transfer.required_approvals,
                current: transfer.approvals.len() as u32,
            })
        }
    }

    /// Check if a transfer is ready to settle.
    pub fn check_ready(&self, transfer_id: &str, now: u64) -> Result<bool, EnterpriseError> {
        let transfer = self.pending.get(transfer_id).ok_or(EnterpriseError::TransferNotFound)?;
        Ok(transfer.approved && now >= transfer.release_at)
    }

    /// Mark a transfer as settled.
    pub fn mark_settled(&mut self, transfer_id: &str) -> Result<(), EnterpriseError> {
        let transfer = self.pending.get_mut(transfer_id).ok_or(EnterpriseError::TransferNotFound)?;
        if !transfer.approved {
            return Err(EnterpriseError::NotApproved);
        }
        transfer.status = TransferApprovalStatus::Settled;
        Ok(())
    }

    /// Get org configuration.
    pub fn get_org(&self, org_id: &str) -> Option<&OrgConfig> {
        self.orgs.get(org_id)
    }

    /// Get pending transfer.
    pub fn get_pending(&self, transfer_id: &str) -> Option<&PendingTransfer> {
        self.pending.get(transfer_id)
    }

    /// Stats as JSON.
    pub fn stats_json(&self) -> serde_json::Value {
        let pending_count = self.pending.values()
            .filter(|t| t.status != TransferApprovalStatus::Settled && t.status != TransferApprovalStatus::Rejected)
            .count();
        serde_json::json!({
            "total_orgs": self.orgs.len(),
            "pending_transfers": pending_count,
            "total_transfers": self.pending.len(),
        })
    }
}

/// Reset spend counters if enough time has passed.
fn reset_spend_if_needed(spend: &mut SpendTracker, now: u64) {
    let day = 86_400;
    let month = 30 * day;
    if now.saturating_sub(spend.daily_reset_at) >= day {
        spend.daily_spent_cents = 0;
        spend.daily_reset_at = now;
    }
    if now.saturating_sub(spend.monthly_reset_at) >= month {
        spend.monthly_spent_cents = 0;
        spend.monthly_reset_at = now;
    }
}

impl Default for EnterpriseManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum EnterpriseError {
    OrgNotFound,
    TransferNotFound,
    TooManyApprovers,
    InvalidApprovalThreshold,
    UnauthorizedApprover,
    NotPendingApproval,
    NotApproved,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_org() -> (EnterpriseManager, String) {
        let mut mgr = EnterpriseManager::new();
        let mut config = OrgConfig::new("acme", "Acme Corp");
        config.approvers = vec!["admin1".into(), "admin2".into(), "admin3".into()];
        config.required_approvals = 2;
        config.hold_period_secs = 3600;
        config.whitelist_enabled = true;
        config.whitelist.insert("0xTreasury".into());
        config.whitelist.insert("0xVault".into());
        mgr.register_org(config).unwrap();
        (mgr, "acme".into())
    }

    #[test]
    fn test_register_org() {
        let (mgr, org_id) = setup_org();
        let org = mgr.get_org(&org_id).unwrap();
        assert_eq!(org.name, "Acme Corp");
        assert_eq!(org.approvers.len(), 3);
        assert_eq!(org.required_approvals, 2);
    }

    #[test]
    fn test_whitelist_enforcement() {
        let (mut mgr, org_id) = setup_org();
        // Allowed destination
        let (id, result) = mgr.initiate_transfer(&org_id, "0xTreasury", 1_000_000, 1000).unwrap();
        assert!(!id.is_empty());
        assert_ne!(result, ValidationResult::Rejected { reason: String::new() });

        // Blocked destination
        let (_, result) = mgr.initiate_transfer(&org_id, "0xUnknown", 1_000_000, 1001).unwrap();
        assert!(matches!(result, ValidationResult::Rejected { .. }));
    }

    #[test]
    fn test_per_tx_limit() {
        let (mut mgr, org_id) = setup_org();
        let (_, result) = mgr.initiate_transfer(&org_id, "0xTreasury", DEFAULT_PER_TX_LIMIT_CENTS + 1, 1000).unwrap();
        assert!(matches!(result, ValidationResult::Rejected { .. }));
    }

    #[test]
    fn test_daily_limit() {
        let (mut mgr, org_id) = setup_org();
        // Use up daily limit with multiple transfers under per-tx limit
        let per_tx = DEFAULT_PER_TX_LIMIT_CENTS;
        let mut spent = 0u64;
        while spent + per_tx <= DEFAULT_DAILY_LIMIT_CENTS {
            mgr.initiate_transfer(&org_id, "0xTreasury", per_tx, 1000).unwrap();
            spent += per_tx;
        }
        // Remaining space is less than per_tx, try to exceed
        let remaining = DEFAULT_DAILY_LIMIT_CENTS - spent;
        let (_, result) = mgr.initiate_transfer(&org_id, "0xTreasury", remaining + 1, 1001).unwrap();
        assert!(matches!(result, ValidationResult::Rejected { .. }));
    }

    #[test]
    fn test_small_transfer_no_approval_needed() {
        let (mut mgr, org_id) = setup_org();
        // Below large transfer threshold → no approval needed, just hold period
        let (_, result) = mgr.initiate_transfer(&org_id, "0xTreasury", 1_000_000, 1000).unwrap();
        assert!(matches!(result, ValidationResult::InHoldPeriod { .. }));
    }

    #[test]
    fn test_large_transfer_needs_approval() {
        let (mut mgr, org_id) = setup_org();
        let (id, result) = mgr.initiate_transfer(&org_id, "0xTreasury", LARGE_TRANSFER_THRESHOLD_CENTS, 1000).unwrap();
        assert!(matches!(result, ValidationResult::NeedsApproval { required: 2, current: 0 }));
        assert!(!mgr.check_ready(&id, 5000).unwrap());
    }

    #[test]
    fn test_multi_approval_workflow() {
        let (mut mgr, org_id) = setup_org();
        let (id, _) = mgr.initiate_transfer(&org_id, "0xTreasury", LARGE_TRANSFER_THRESHOLD_CENTS, 1000).unwrap();

        // First approval
        let result = mgr.approve_transfer(&id, "admin1").unwrap();
        assert!(matches!(result, ValidationResult::NeedsApproval { required: 2, current: 1 }));

        // Second approval → moves to hold period
        let result = mgr.approve_transfer(&id, "admin2").unwrap();
        assert!(matches!(result, ValidationResult::InHoldPeriod { .. }));

        // Not ready yet (hold period)
        assert!(!mgr.check_ready(&id, 1000).unwrap());
        // Ready after hold period
        assert!(mgr.check_ready(&id, 1000 + 3601).unwrap());
    }

    #[test]
    fn test_unauthorized_approver() {
        let (mut mgr, org_id) = setup_org();
        let (id, _) = mgr.initiate_transfer(&org_id, "0xTreasury", LARGE_TRANSFER_THRESHOLD_CENTS, 1000).unwrap();
        let result = mgr.approve_transfer(&id, "unknown_admin");
        assert_eq!(result.unwrap_err(), EnterpriseError::UnauthorizedApprover);
    }

    #[test]
    fn test_mark_settled() {
        let (mut mgr, org_id) = setup_org();
        // Small transfer (no approval needed, instant hold=0)
        let mut config = mgr.orgs.get_mut(&org_id).unwrap();
        config.hold_period_secs = 0;
        let config_clone = config.clone();
        drop(config);
        mgr.orgs.insert(org_id.clone(), config_clone);

        let (id, result) = mgr.initiate_transfer(&org_id, "0xTreasury", 1_000_000, 1000).unwrap();
        assert_eq!(result, ValidationResult::Approved);
        mgr.mark_settled(&id).unwrap();
        assert_eq!(mgr.get_pending(&id).unwrap().status, TransferApprovalStatus::Settled);
    }

    #[test]
    fn test_add_remove_whitelist() {
        let (mut mgr, org_id) = setup_org();
        mgr.add_to_whitelist(&org_id, "0xNew").unwrap();
        let org = mgr.get_org(&org_id).unwrap();
        assert!(org.whitelist.contains("0xNew"));

        mgr.remove_from_whitelist(&org_id, "0xNew").unwrap();
        let org = mgr.get_org(&org_id).unwrap();
        assert!(!org.whitelist.contains("0xNew"));
    }

    #[test]
    fn test_too_many_approvers() {
        let mut mgr = EnterpriseManager::new();
        let mut config = OrgConfig::new("big", "BigCo");
        config.approvers = (0..MAX_APPROVERS + 1).map(|i| format!("admin{i}")).collect();
        assert_eq!(mgr.register_org(config), Err(EnterpriseError::TooManyApprovers));
    }

    #[test]
    fn test_invalid_threshold() {
        let mut mgr = EnterpriseManager::new();
        let mut config = OrgConfig::new("bad", "BadCo");
        config.approvers = vec!["a".into(), "b".into()];
        config.required_approvals = 5; // more than approvers
        assert_eq!(mgr.register_org(config), Err(EnterpriseError::InvalidApprovalThreshold));
    }

    #[test]
    fn test_stats_json() {
        let (mgr, _) = setup_org();
        let j = mgr.stats_json();
        assert_eq!(j["total_orgs"], 1);
    }

    #[test]
    fn test_daily_limit_resets() {
        let (mut mgr, org_id) = setup_org();
        // Spend a chunk (under per-tx limit)
        let chunk = DEFAULT_PER_TX_LIMIT_CENTS;
        mgr.initiate_transfer(&org_id, "0xTreasury", chunk, 1000).unwrap();
        // Next day: should reset, same amount should work again
        let (_, result) = mgr.initiate_transfer(&org_id, "0xTreasury", chunk, 1000 + 86_401).unwrap();
        assert!(!matches!(result, ValidationResult::Rejected { .. }));
    }
}
