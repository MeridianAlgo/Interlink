/// Bug bounty program registry for InterLink (Phase 9)
///
/// On-chain bounty system with severity levels, reward ranges, and submission
/// lifecycle management. Integrates with governance for payout approval.
///
/// Reward tiers (competitive with Wormhole's $50k-$2M program):
///   Critical: $100k - $500k (e.g., proof forgery, fund theft)
///   High:     $10k  - $100k (e.g., validator bypass, replay)
///   Medium:   $1k   - $10k  (e.g., DoS, incorrect fee calculation)
///   Low:      $100  - $1k   (e.g., cosmetic, documentation)

use std::collections::HashMap;

// ─── Constants ───────────────────────────────────────────────────────────────

pub const CRITICAL_MIN: u64 = 100_000;
pub const CRITICAL_MAX: u64 = 500_000;
pub const HIGH_MIN: u64 = 10_000;
pub const HIGH_MAX: u64 = 100_000;
pub const MEDIUM_MIN: u64 = 1_000;
pub const MEDIUM_MAX: u64 = 10_000;
pub const LOW_MIN: u64 = 100;
pub const LOW_MAX: u64 = 1_000;

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn reward_range(&self) -> (u64, u64) {
        match self {
            Severity::Critical => (CRITICAL_MIN, CRITICAL_MAX),
            Severity::High => (HIGH_MIN, HIGH_MAX),
            Severity::Medium => (MEDIUM_MIN, MEDIUM_MAX),
            Severity::Low => (LOW_MIN, LOW_MAX),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Severity::Critical => "CRITICAL",
            Severity::High => "HIGH",
            Severity::Medium => "MEDIUM",
            Severity::Low => "LOW",
        }
    }

    /// Max response time in hours
    pub fn response_sla_hours(&self) -> u32 {
        match self {
            Severity::Critical => 4,
            Severity::High => 24,
            Severity::Medium => 72,
            Severity::Low => 168, // 1 week
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionStatus {
    /// Submitted, awaiting triage
    Pending,
    /// Under review by security team
    Triaging,
    /// Confirmed as valid vulnerability
    Confirmed,
    /// Reward amount determined, awaiting governance approval
    RewardPending { amount: u64 },
    /// Reward paid out
    Paid { amount: u64 },
    /// Not a valid vulnerability
    Rejected { reason: String },
    /// Duplicate of an existing report
    Duplicate { original_id: u64 },
}

#[derive(Debug, Clone)]
pub struct Submission {
    pub id: u64,
    pub reporter: String,
    pub severity: Severity,
    pub title: String,
    pub description: String,
    pub proof_of_concept: String,
    pub status: SubmissionStatus,
    pub submitted_at: u64,
    pub resolved_at: Option<u64>,
    /// Affected module/contract
    pub affected_component: String,
}

// ─── Bounty Registry ─────────────────────────────────────────────────────────

pub struct BountyRegistry {
    submissions: HashMap<u64, Submission>,
    next_id: u64,
    /// Total paid out across all bounties
    pub total_paid: u64,
    /// Total submissions received
    pub total_submissions: u64,
}

impl BountyRegistry {
    pub fn new() -> Self {
        BountyRegistry {
            submissions: HashMap::new(),
            next_id: 1,
            total_paid: 0,
            total_submissions: 0,
        }
    }

    /// Submit a new bug report.
    pub fn submit(
        &mut self,
        reporter: impl Into<String>,
        severity: Severity,
        title: impl Into<String>,
        description: impl Into<String>,
        proof_of_concept: impl Into<String>,
        affected_component: impl Into<String>,
        now: u64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.total_submissions += 1;

        let submission = Submission {
            id,
            reporter: reporter.into(),
            severity,
            title: title.into(),
            description: description.into(),
            proof_of_concept: proof_of_concept.into(),
            status: SubmissionStatus::Pending,
            submitted_at: now,
            resolved_at: None,
            affected_component: affected_component.into(),
        };
        self.submissions.insert(id, submission);
        id
    }

    /// Move a submission to triaging.
    pub fn triage(&mut self, id: u64) -> Result<(), BountyError> {
        let sub = self.get_mut(id)?;
        if sub.status != SubmissionStatus::Pending {
            return Err(BountyError::InvalidTransition {
                from: format!("{:?}", sub.status),
                to: "Triaging".into(),
            });
        }
        sub.status = SubmissionStatus::Triaging;
        Ok(())
    }

    /// Confirm a vulnerability and set a reward amount.
    pub fn confirm(
        &mut self,
        id: u64,
        reward_amount: u64,
    ) -> Result<(), BountyError> {
        let sub = self.get_mut(id)?;
        if sub.status != SubmissionStatus::Triaging {
            return Err(BountyError::InvalidTransition {
                from: format!("{:?}", sub.status),
                to: "Confirmed".into(),
            });
        }
        let (min, max) = sub.severity.reward_range();
        if reward_amount < min || reward_amount > max {
            return Err(BountyError::RewardOutOfRange {
                amount: reward_amount,
                min,
                max,
            });
        }
        sub.status = SubmissionStatus::Confirmed;
        sub.status = SubmissionStatus::RewardPending { amount: reward_amount };
        Ok(())
    }

    /// Pay out the reward (after governance approval).
    pub fn pay(&mut self, id: u64, now: u64) -> Result<u64, BountyError> {
        let sub = self.get_mut(id)?;
        let amount = match &sub.status {
            SubmissionStatus::RewardPending { amount } => *amount,
            _ => {
                return Err(BountyError::InvalidTransition {
                    from: format!("{:?}", sub.status),
                    to: "Paid".into(),
                });
            }
        };
        sub.status = SubmissionStatus::Paid { amount };
        sub.resolved_at = Some(now);
        self.total_paid += amount;
        Ok(amount)
    }

    /// Reject a submission.
    pub fn reject(
        &mut self,
        id: u64,
        reason: impl Into<String>,
        now: u64,
    ) -> Result<(), BountyError> {
        let sub = self.get_mut(id)?;
        sub.status = SubmissionStatus::Rejected { reason: reason.into() };
        sub.resolved_at = Some(now);
        Ok(())
    }

    /// Mark as duplicate of another submission.
    pub fn mark_duplicate(
        &mut self,
        id: u64,
        original_id: u64,
        now: u64,
    ) -> Result<(), BountyError> {
        if !self.submissions.contains_key(&original_id) {
            return Err(BountyError::SubmissionNotFound { id: original_id });
        }
        let sub = self.get_mut(id)?;
        sub.status = SubmissionStatus::Duplicate { original_id };
        sub.resolved_at = Some(now);
        Ok(())
    }

    /// Get a submission by ID.
    pub fn get(&self, id: u64) -> Option<&Submission> {
        self.submissions.get(&id)
    }

    fn get_mut(&mut self, id: u64) -> Result<&mut Submission, BountyError> {
        self.submissions
            .get_mut(&id)
            .ok_or(BountyError::SubmissionNotFound { id })
    }

    /// All submissions with a given status type.
    pub fn by_status_pending(&self) -> Vec<&Submission> {
        self.submissions
            .values()
            .filter(|s| matches!(s.status, SubmissionStatus::Pending))
            .collect()
    }

    /// All submissions for a given severity.
    pub fn by_severity(&self, severity: &Severity) -> Vec<&Submission> {
        self.submissions
            .values()
            .filter(|s| s.severity == *severity)
            .collect()
    }

    /// Statistics snapshot.
    pub fn stats(&self) -> BountyStats {
        let mut by_severity = HashMap::new();
        let mut by_status = HashMap::new();
        for sub in self.submissions.values() {
            *by_severity.entry(sub.severity.name()).or_insert(0u64) += 1;
            let status_name = match &sub.status {
                SubmissionStatus::Pending => "pending",
                SubmissionStatus::Triaging => "triaging",
                SubmissionStatus::Confirmed => "confirmed",
                SubmissionStatus::RewardPending { .. } => "reward_pending",
                SubmissionStatus::Paid { .. } => "paid",
                SubmissionStatus::Rejected { .. } => "rejected",
                SubmissionStatus::Duplicate { .. } => "duplicate",
            };
            *by_status.entry(status_name).or_insert(0u64) += 1;
        }
        BountyStats {
            total_submissions: self.total_submissions,
            total_paid: self.total_paid,
            by_severity,
            by_status,
        }
    }
}

impl Default for BountyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct BountyStats {
    pub total_submissions: u64,
    pub total_paid: u64,
    pub by_severity: HashMap<&'static str, u64>,
    pub by_status: HashMap<&'static str, u64>,
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum BountyError {
    SubmissionNotFound { id: u64 },
    InvalidTransition { from: String, to: String },
    RewardOutOfRange { amount: u64, min: u64, max: u64 },
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_with_submission() -> (BountyRegistry, u64) {
        let mut reg = BountyRegistry::new();
        let id = reg.submit(
            "alice@security.com",
            Severity::High,
            "Replay attack on multisig bundle",
            "A validator can replay a signed bundle from epoch N in epoch N+1",
            "See attached PoC script that replays tx hash 0xabc...",
            "multisig.rs",
            1000,
        );
        (reg, id)
    }

    #[test]
    fn test_severity_reward_ranges() {
        assert_eq!(Severity::Critical.reward_range(), (100_000, 500_000));
        assert_eq!(Severity::High.reward_range(), (10_000, 100_000));
        assert_eq!(Severity::Medium.reward_range(), (1_000, 10_000));
        assert_eq!(Severity::Low.reward_range(), (100, 1_000));
    }

    #[test]
    fn test_severity_response_sla() {
        assert_eq!(Severity::Critical.response_sla_hours(), 4);
        assert_eq!(Severity::High.response_sla_hours(), 24);
    }

    #[test]
    fn test_submit_and_retrieve() {
        let (reg, id) = registry_with_submission();
        let sub = reg.get(id).unwrap();
        assert_eq!(sub.reporter, "alice@security.com");
        assert_eq!(sub.severity, Severity::High);
        assert!(matches!(sub.status, SubmissionStatus::Pending));
    }

    #[test]
    fn test_full_lifecycle_happy_path() {
        let (mut reg, id) = registry_with_submission();
        reg.triage(id).unwrap();
        reg.confirm(id, 50_000).unwrap(); // $50k within HIGH range
        let paid = reg.pay(id, 2000).unwrap();
        assert_eq!(paid, 50_000);
        assert_eq!(reg.total_paid, 50_000);
        let sub = reg.get(id).unwrap();
        assert!(matches!(sub.status, SubmissionStatus::Paid { amount: 50_000 }));
        assert_eq!(sub.resolved_at, Some(2000));
    }

    #[test]
    fn test_reward_out_of_range() {
        let (mut reg, id) = registry_with_submission();
        reg.triage(id).unwrap();
        // HIGH range is $10k-$100k, try $500k
        let err = reg.confirm(id, 500_000).unwrap_err();
        assert_eq!(
            err,
            BountyError::RewardOutOfRange { amount: 500_000, min: HIGH_MIN, max: HIGH_MAX }
        );
    }

    #[test]
    fn test_invalid_transition() {
        let (mut reg, id) = registry_with_submission();
        // Can't confirm directly from Pending (need Triaging first)
        let err = reg.confirm(id, 50_000).unwrap_err();
        assert!(matches!(err, BountyError::InvalidTransition { .. }));
    }

    #[test]
    fn test_reject_submission() {
        let (mut reg, id) = registry_with_submission();
        reg.reject(id, "Not a vulnerability — expected behavior", 1500).unwrap();
        let sub = reg.get(id).unwrap();
        assert!(matches!(sub.status, SubmissionStatus::Rejected { .. }));
    }

    #[test]
    fn test_mark_duplicate() {
        let mut reg = BountyRegistry::new();
        let id1 = reg.submit("alice", Severity::Medium, "t1", "d1", "p1", "fee.rs", 100);
        let id2 = reg.submit("bob", Severity::Medium, "t2", "d2", "p2", "fee.rs", 200);
        reg.mark_duplicate(id2, id1, 300).unwrap();
        let sub = reg.get(id2).unwrap();
        assert_eq!(sub.status, SubmissionStatus::Duplicate { original_id: id1 });
    }

    #[test]
    fn test_stats() {
        let mut reg = BountyRegistry::new();
        reg.submit("a", Severity::Critical, "t", "d", "p", "c", 0);
        reg.submit("b", Severity::High, "t", "d", "p", "c", 0);
        reg.submit("c", Severity::High, "t", "d", "p", "c", 0);
        let stats = reg.stats();
        assert_eq!(stats.total_submissions, 3);
        assert_eq!(stats.by_severity["CRITICAL"], 1);
        assert_eq!(stats.by_severity["HIGH"], 2);
        assert_eq!(stats.by_status["pending"], 3);
    }

    #[test]
    fn test_by_severity_filter() {
        let mut reg = BountyRegistry::new();
        reg.submit("a", Severity::Low, "t", "d", "p", "c", 0);
        reg.submit("b", Severity::Critical, "t", "d", "p", "c", 0);
        reg.submit("c", Severity::Low, "t", "d", "p", "c", 0);
        assert_eq!(reg.by_severity(&Severity::Low).len(), 2);
        assert_eq!(reg.by_severity(&Severity::Critical).len(), 1);
    }

    #[test]
    fn test_submission_not_found() {
        let mut reg = BountyRegistry::new();
        let err = reg.triage(999).unwrap_err();
        assert_eq!(err, BountyError::SubmissionNotFound { id: 999 });
    }
}
