/// AML/KYC integration for InterLink (Phase 12, optional, community-governed).
///
/// This module provides optional AML/KYC screening that can be enabled or
/// disabled by governance vote. When disabled, the bridge is fully permissionless.
/// When enabled, transfers from blocked addresses are rejected at the relayer level.
///
/// Design principles:
///   - Community-governed: the active sanctions list is voted in by DAO (governance.rs)
///   - Privacy-preserving: KYC attestations are hashed; raw PII never stored on-chain
///   - Permissionless by default: `KycRegistry::new()` starts with screening disabled
///   - Auditable: all screening decisions are logged with reason codes
///
/// Comparison:
///   Wormhole:  no built-in KYC/AML
///   Across:    no built-in KYC/AML
///   Stargate:  no built-in KYC/AML
///   Circle CCTP: mandatory KYC for USDC transfers
///   InterLink: optional, community-governed, privacy-preserving
use std::collections::{HashMap, HashSet};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Maximum number of entries in the community sanctions list.
pub const MAX_SANCTIONS_ENTRIES: usize = 10_000;
/// Maximum number of approved KYC attestations.
pub const MAX_KYC_RECORDS: usize = 1_000_000;
/// Risk score threshold above which a transfer is automatically flagged (0-100).
pub const AUTO_FLAG_RISK_THRESHOLD: u8 = 75;
/// Risk score threshold above which a transfer is automatically blocked (0-100).
pub const AUTO_BLOCK_RISK_THRESHOLD: u8 = 90;

// ─── Types ────────────────────────────────────────────────────────────────────

/// Risk level assigned to an address or transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// No known risk — proceed normally.
    Low,
    /// Moderate risk — log and monitor but allow.
    Medium,
    /// High risk — flag for manual review.
    High,
    /// Blocked — reject the transfer.
    Blocked,
}

impl RiskLevel {
    fn from_score(score: u8) -> Self {
        if score >= AUTO_BLOCK_RISK_THRESHOLD {
            RiskLevel::Blocked
        } else if score >= AUTO_FLAG_RISK_THRESHOLD {
            RiskLevel::High
        } else if score >= 40 {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        }
    }
}

/// A sanctioned address entry, added by community governance.
#[derive(Debug, Clone)]
pub struct SanctionEntry {
    /// Hex-encoded address (EVM or Solana base58).
    pub address: String,
    /// Reason code for the sanction.
    pub reason: SanctionReason,
    /// UNIX timestamp when this entry was added.
    pub added_at: u64,
    /// Governance proposal ID that added this entry.
    pub proposal_id: u64,
}

/// Reason codes for sanctions — mirrors OFAC / FATF categories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SanctionReason {
    /// Address appears on OFAC SDN list.
    OfacSdn,
    /// Address linked to known exploit / hack.
    ExploitRelated,
    /// Address linked to ransomware payments.
    Ransomware,
    /// Address used for bridge sandwich attacks.
    SandwichAttack,
    /// Community-flagged, pending review.
    CommunityFlag,
    /// Custom reason provided by governance vote.
    Custom(String),
}

/// A KYC attestation for an address. Raw PII is never stored.
/// The relayer stores only the hash of the attestation document.
#[derive(Debug, Clone)]
pub struct KycRecord {
    /// Hex-encoded address.
    pub address: String,
    /// SHA-256 hash of the off-chain KYC attestation document.
    pub attestation_hash: [u8; 32],
    /// KYC provider identifier (e.g., "jumio", "sumsub", "fractal").
    pub provider: String,
    /// Risk score assigned by the KYC provider (0 = lowest, 100 = highest).
    pub risk_score: u8,
    /// UNIX timestamp of the attestation.
    pub attested_at: u64,
    /// UNIX timestamp when this record expires (re-KYC required after this).
    pub expires_at: u64,
}

impl KycRecord {
    /// Returns true if the record is still valid at the given timestamp.
    pub fn is_valid_at(&self, now: u64) -> bool {
        now < self.expires_at
    }

    /// Risk level derived from the provider's score.
    pub fn risk_level(&self) -> RiskLevel {
        RiskLevel::from_score(self.risk_score)
    }
}

/// Result of an AML screening check.
#[derive(Debug, Clone)]
pub struct ScreeningResult {
    /// Address that was screened.
    pub address: String,
    /// Whether the transfer should be blocked.
    pub blocked: bool,
    /// Whether the transfer should be flagged for manual review.
    pub flagged: bool,
    /// Risk level determined by the screener.
    pub risk_level: RiskLevel,
    /// Human-readable reason for the decision.
    pub reason: String,
}

impl ScreeningResult {
    fn allow(address: impl Into<String>) -> Self {
        ScreeningResult {
            address: address.into(),
            blocked: false,
            flagged: false,
            risk_level: RiskLevel::Low,
            reason: "address not on sanctions list".into(),
        }
    }

    fn block(address: impl Into<String>, reason: impl Into<String>) -> Self {
        ScreeningResult {
            address: address.into(),
            blocked: true,
            flagged: true,
            risk_level: RiskLevel::Blocked,
            reason: reason.into(),
        }
    }

    fn flag(address: impl Into<String>, reason: impl Into<String>, risk: RiskLevel) -> Self {
        ScreeningResult {
            address: address.into(),
            blocked: false,
            flagged: true,
            risk_level: risk,
            reason: reason.into(),
        }
    }
}

// ─── KycRegistry ─────────────────────────────────────────────────────────────

/// Community-governed KYC/AML registry.
///
/// Starts in permissionless mode (screening disabled). Governance can vote to
/// enable screening and manage the sanctions list.
#[derive(Debug, Default)]
pub struct KycRegistry {
    /// Whether screening is currently active (set by governance).
    screening_enabled: bool,
    /// Sanctioned addresses (blocked unconditionally when screening is active).
    sanctions: HashMap<String, SanctionEntry>,
    /// Approved KYC records (when require_kyc=true, only these addresses may bridge).
    kyc_records: HashMap<String, KycRecord>,
    /// Whether all senders must have a valid KYC record to bridge.
    require_kyc: bool,
    /// Addresses explicitly whitelisted regardless of screening.
    whitelist: HashSet<String>,
}

impl KycRegistry {
    /// Create a new registry with screening disabled (permissionless mode).
    pub fn new() -> Self {
        KycRegistry::default()
    }

    /// Enable or disable AML screening (called by governance).
    pub fn set_screening_enabled(&mut self, enabled: bool) {
        self.screening_enabled = enabled;
    }

    /// Enable or disable mandatory KYC for all senders (called by governance).
    pub fn set_require_kyc(&mut self, required: bool) {
        self.require_kyc = required;
    }

    /// Returns true if screening is currently active.
    pub fn screening_enabled(&self) -> bool {
        self.screening_enabled
    }

    /// Add an address to the sanctions list (governance-gated in production).
    pub fn add_sanction(
        &mut self,
        address: impl Into<String>,
        reason: SanctionReason,
        added_at: u64,
        proposal_id: u64,
    ) -> Result<(), KycError> {
        if self.sanctions.len() >= MAX_SANCTIONS_ENTRIES {
            return Err(KycError::SanctionsListFull);
        }
        let address = address.into();
        self.sanctions.insert(
            address.clone(),
            SanctionEntry {
                address,
                reason,
                added_at,
                proposal_id,
            },
        );
        Ok(())
    }

    /// Remove an address from the sanctions list (governance-gated in production).
    pub fn remove_sanction(&mut self, address: &str) -> bool {
        self.sanctions.remove(address).is_some()
    }

    /// Register a KYC attestation for an address.
    pub fn register_kyc(&mut self, record: KycRecord) -> Result<(), KycError> {
        if self.kyc_records.len() >= MAX_KYC_RECORDS {
            return Err(KycError::KycRegistryFull);
        }
        self.kyc_records.insert(record.address.clone(), record);
        Ok(())
    }

    /// Add an address to the whitelist (always allowed, even if sanctioned).
    pub fn whitelist(&mut self, address: impl Into<String>) {
        self.whitelist.insert(address.into());
    }

    /// Screen a sender address for AML/KYC compliance.
    ///
    /// Returns a `ScreeningResult` indicating whether the transfer should be
    /// blocked, flagged, or allowed. When `screening_enabled = false`, always
    /// returns allow.
    pub fn screen(&self, address: &str, now: u64) -> ScreeningResult {
        // Permissionless mode: skip all checks.
        if !self.screening_enabled {
            return ScreeningResult::allow(address);
        }

        // Whitelisted addresses always pass.
        if self.whitelist.contains(address) {
            return ScreeningResult::allow(address);
        }

        // Check sanctions list.
        if let Some(entry) = self.sanctions.get(address) {
            let reason = format!(
                "address on sanctions list: {:?} (proposal #{})",
                entry.reason, entry.proposal_id
            );
            return ScreeningResult::block(address, reason);
        }

        // If mandatory KYC is required, check for valid record.
        if self.require_kyc {
            match self.kyc_records.get(address) {
                None => {
                    return ScreeningResult::block(
                        address,
                        "mandatory KYC required: no record found",
                    );
                }
                Some(record) if !record.is_valid_at(now) => {
                    return ScreeningResult::block(
                        address,
                        "mandatory KYC required: record expired",
                    );
                }
                Some(record) => {
                    let risk = record.risk_level();
                    if risk == RiskLevel::Blocked {
                        return ScreeningResult::block(
                            address,
                            format!(
                                "KYC risk score {} exceeds block threshold",
                                record.risk_score
                            ),
                        );
                    }
                    if risk >= RiskLevel::High {
                        return ScreeningResult::flag(
                            address,
                            format!(
                                "KYC risk score {} exceeds flag threshold",
                                record.risk_score
                            ),
                            risk,
                        );
                    }
                }
            }
        } else {
            // Optional KYC: if a record exists, factor in the risk score.
            if let Some(record) = self.kyc_records.get(address) {
                if record.is_valid_at(now) {
                    let risk = record.risk_level();
                    if risk == RiskLevel::Blocked {
                        return ScreeningResult::block(
                            address,
                            format!(
                                "KYC risk score {} exceeds block threshold",
                                record.risk_score
                            ),
                        );
                    }
                    if risk >= RiskLevel::High {
                        return ScreeningResult::flag(
                            address,
                            format!(
                                "KYC risk score {} exceeds flag threshold",
                                record.risk_score
                            ),
                            risk,
                        );
                    }
                }
            }
        }

        ScreeningResult::allow(address)
    }

    /// Returns the number of entries in the sanctions list.
    pub fn sanctions_count(&self) -> usize {
        self.sanctions.len()
    }

    /// Returns the number of KYC records.
    pub fn kyc_count(&self) -> usize {
        self.kyc_records.len()
    }

    /// Returns whether an address is currently sanctioned.
    pub fn is_sanctioned(&self, address: &str) -> bool {
        self.sanctions.contains_key(address)
    }

    /// Returns the KYC record for an address, if any.
    pub fn get_kyc_record(&self, address: &str) -> Option<&KycRecord> {
        self.kyc_records.get(address)
    }
}

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum KycError {
    SanctionsListFull,
    KycRegistryFull,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;
    const FUTURE: u64 = NOW + 365 * 24 * 3600;

    fn kyc_record(address: &str, risk_score: u8) -> KycRecord {
        KycRecord {
            address: address.into(),
            attestation_hash: [0xAB; 32],
            provider: "test-provider".into(),
            risk_score,
            attested_at: NOW,
            expires_at: FUTURE,
        }
    }

    #[test]
    fn permissionless_mode_always_allows() {
        let reg = KycRegistry::new();
        // Screening disabled by default — even sanctioned addresses pass
        let result = reg.screen("0xbadactor", NOW);
        assert!(!result.blocked);
        assert_eq!(result.risk_level, RiskLevel::Low);
    }

    #[test]
    fn sanctioned_address_blocked_when_screening_on() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        reg.add_sanction("0xhacker", SanctionReason::ExploitRelated, NOW, 1)
            .unwrap();

        let result = reg.screen("0xhacker", NOW);
        assert!(result.blocked);
        assert_eq!(result.risk_level, RiskLevel::Blocked);
        assert!(result.reason.contains("sanctions list"));
    }

    #[test]
    fn unknown_address_allowed_without_mandatory_kyc() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);

        let result = reg.screen("0xunknown", NOW);
        assert!(!result.blocked);
    }

    #[test]
    fn mandatory_kyc_blocks_unregistered_sender() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        reg.set_require_kyc(true);

        let result = reg.screen("0xunregistered", NOW);
        assert!(result.blocked);
        assert!(result.reason.contains("no record found"));
    }

    #[test]
    fn mandatory_kyc_allows_low_risk_registered_sender() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        reg.set_require_kyc(true);
        reg.register_kyc(kyc_record("0xalice", 10)).unwrap();

        let result = reg.screen("0xalice", NOW);
        assert!(!result.blocked);
        assert!(!result.flagged);
    }

    #[test]
    fn mandatory_kyc_flags_high_risk_sender() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        reg.set_require_kyc(true);
        reg.register_kyc(kyc_record("0xrisky", 80)).unwrap();

        let result = reg.screen("0xrisky", NOW);
        assert!(!result.blocked, "high-risk should be flagged, not blocked");
        assert!(result.flagged);
        assert_eq!(result.risk_level, RiskLevel::High);
    }

    #[test]
    fn mandatory_kyc_blocks_very_high_risk_sender() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        reg.set_require_kyc(true);
        reg.register_kyc(kyc_record("0xdanger", 95)).unwrap();

        let result = reg.screen("0xdanger", NOW);
        assert!(result.blocked);
        assert_eq!(result.risk_level, RiskLevel::Blocked);
    }

    #[test]
    fn expired_kyc_record_blocks_when_mandatory() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        reg.set_require_kyc(true);

        let expired = KycRecord {
            address: "0xbob".into(),
            attestation_hash: [0x00; 32],
            provider: "test".into(),
            risk_score: 5,
            attested_at: 1_000,
            expires_at: 2_000, // expired long ago
        };
        reg.register_kyc(expired).unwrap();

        let result = reg.screen("0xbob", NOW);
        assert!(result.blocked);
        assert!(result.reason.contains("expired"));
    }

    #[test]
    fn whitelist_bypasses_sanctions() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        reg.add_sanction("0xcustody", SanctionReason::OfacSdn, NOW, 2)
            .unwrap();
        reg.whitelist("0xcustody");

        let result = reg.screen("0xcustody", NOW);
        assert!(!result.blocked, "whitelisted address must bypass sanctions");
    }

    #[test]
    fn remove_sanction_re_allows_address() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        reg.add_sanction("0xtemp", SanctionReason::CommunityFlag, NOW, 3)
            .unwrap();
        assert!(reg.screen("0xtemp", NOW).blocked);

        reg.remove_sanction("0xtemp");
        assert!(!reg.screen("0xtemp", NOW).blocked);
    }

    #[test]
    fn risk_level_thresholds_correct() {
        assert_eq!(RiskLevel::from_score(0), RiskLevel::Low);
        assert_eq!(RiskLevel::from_score(39), RiskLevel::Low);
        assert_eq!(RiskLevel::from_score(40), RiskLevel::Medium);
        assert_eq!(RiskLevel::from_score(74), RiskLevel::Medium);
        assert_eq!(RiskLevel::from_score(75), RiskLevel::High);
        assert_eq!(RiskLevel::from_score(89), RiskLevel::High);
        assert_eq!(RiskLevel::from_score(90), RiskLevel::Blocked);
        assert_eq!(RiskLevel::from_score(100), RiskLevel::Blocked);
    }

    #[test]
    fn sanctions_list_capacity_enforced() {
        let mut reg = KycRegistry::new();
        for i in 0..MAX_SANCTIONS_ENTRIES {
            reg.add_sanction(
                format!("0x{:040x}", i),
                SanctionReason::CommunityFlag,
                NOW,
                i as u64,
            )
            .unwrap();
        }
        let err = reg
            .add_sanction("0xoverflow", SanctionReason::CommunityFlag, NOW, 99999)
            .unwrap_err();
        assert_eq!(err, KycError::SanctionsListFull);
    }

    #[test]
    fn optional_kyc_risk_score_applied_when_record_exists() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        // require_kyc = false (default)
        reg.register_kyc(kyc_record("0xwatched", 92)).unwrap();

        let result = reg.screen("0xwatched", NOW);
        assert!(
            result.blocked,
            "optional KYC with score 92 must still block"
        );
    }

    #[test]
    fn optional_kyc_no_record_still_allows() {
        let mut reg = KycRegistry::new();
        reg.set_screening_enabled(true);
        // No KYC record, require_kyc = false

        let result = reg.screen("0xanon", NOW);
        assert!(
            !result.blocked,
            "unknown address without mandatory KYC must pass"
        );
    }
}
