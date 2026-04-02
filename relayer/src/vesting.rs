/// Token vesting schedules for InterLink (Phase 9)
///
/// Manages team, advisor, and treasury token unlock schedules.
/// Supports cliff + linear vesting, revocation, and multi-beneficiary tracking.
///
/// Vesting schedules:
///   - Team (30%):     4-year vest, 1-year cliff, monthly linear unlock
///   - Advisors:       2-year vest, 6-month cliff, quarterly unlock
///   - Treasury (30%): No cliff, 3-year linear unlock (DAO-governed)
///   - Community (40%): Immediate (liquidity mining, airdrops, grants)
///
/// Comparison:
///   Wormhole:  guardian staking, no token vesting
///   Stargate:  STG 3yr vesting for team, similar cliff
///   Across:    UMA-based, simpler vesting
///   InterLink: granular per-beneficiary with revocation + DAO governance
use std::collections::HashMap;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Team vesting duration (4 years in seconds).
pub const TEAM_VEST_DURATION_SECS: u64 = 4 * 365 * 24 * 3600;
/// Team cliff duration (1 year in seconds).
pub const TEAM_CLIFF_SECS: u64 = 365 * 24 * 3600;
/// Advisor vesting duration (2 years).
pub const ADVISOR_VEST_DURATION_SECS: u64 = 2 * 365 * 24 * 3600;
/// Advisor cliff duration (6 months).
pub const ADVISOR_CLIFF_SECS: u64 = 180 * 24 * 3600;
/// Treasury vesting duration (3 years, no cliff).
pub const TREASURY_VEST_DURATION_SECS: u64 = 3 * 365 * 24 * 3600;

// ─── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VestingCategory {
    Team,
    Advisor,
    Treasury,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleStatus {
    Active,
    Revoked { revoked_at: u64 },
    FullyVested,
}

/// A vesting schedule for a single beneficiary.
#[derive(Debug, Clone)]
pub struct VestingSchedule {
    /// Beneficiary identifier (address).
    pub beneficiary: String,
    /// Category of vesting.
    pub category: VestingCategory,
    /// Total tokens allocated.
    pub total_amount: u64,
    /// Tokens already claimed.
    pub claimed: u64,
    /// Vesting start timestamp.
    pub start_time: u64,
    /// Cliff end timestamp (no tokens vest before this).
    pub cliff_end: u64,
    /// Vesting end timestamp (all tokens vested by this time).
    pub vest_end: u64,
    /// Schedule status.
    pub status: ScheduleStatus,
}

impl VestingSchedule {
    /// Create a new vesting schedule.
    pub fn new(
        beneficiary: impl Into<String>,
        category: VestingCategory,
        total_amount: u64,
        start_time: u64,
        cliff_duration: u64,
        vest_duration: u64,
    ) -> Self {
        VestingSchedule {
            beneficiary: beneficiary.into(),
            category,
            total_amount,
            claimed: 0,
            start_time,
            cliff_end: start_time + cliff_duration,
            vest_end: start_time + vest_duration,
            status: ScheduleStatus::Active,
        }
    }

    /// Compute total vested amount at timestamp `now`.
    pub fn vested_at(&self, now: u64) -> u64 {
        match &self.status {
            ScheduleStatus::Revoked { revoked_at } => {
                // Only vest up to revocation time
                self.compute_vested(*revoked_at)
            }
            ScheduleStatus::FullyVested => self.total_amount,
            ScheduleStatus::Active => self.compute_vested(now),
        }
    }

    /// Compute claimable (vested - already claimed).
    pub fn claimable_at(&self, now: u64) -> u64 {
        self.vested_at(now).saturating_sub(self.claimed)
    }

    fn compute_vested(&self, now: u64) -> u64 {
        if now < self.cliff_end {
            return 0;
        }
        if now >= self.vest_end {
            return self.total_amount;
        }
        let elapsed = now - self.start_time;
        let duration = self.vest_end - self.start_time;
        (self.total_amount as u128 * elapsed as u128 / duration as u128) as u64
    }
}

// ─── Vesting Registry ───────────────────────────────────────────────────────

pub struct VestingRegistry {
    schedules: Vec<VestingSchedule>,
    /// Index: beneficiary → schedule indices.
    by_beneficiary: HashMap<String, Vec<usize>>,
}

impl VestingRegistry {
    pub fn new() -> Self {
        VestingRegistry {
            schedules: Vec::new(),
            by_beneficiary: HashMap::new(),
        }
    }

    /// Add a new vesting schedule. Returns the schedule index.
    pub fn add_schedule(&mut self, schedule: VestingSchedule) -> usize {
        let idx = self.schedules.len();
        self.by_beneficiary
            .entry(schedule.beneficiary.clone())
            .or_default()
            .push(idx);
        self.schedules.push(schedule);
        idx
    }

    /// Create a standard team vesting schedule.
    pub fn add_team_schedule(
        &mut self,
        beneficiary: impl Into<String>,
        amount: u64,
        start_time: u64,
    ) -> usize {
        let schedule = VestingSchedule::new(
            beneficiary,
            VestingCategory::Team,
            amount,
            start_time,
            TEAM_CLIFF_SECS,
            TEAM_VEST_DURATION_SECS,
        );
        self.add_schedule(schedule)
    }

    /// Create a standard advisor vesting schedule.
    pub fn add_advisor_schedule(
        &mut self,
        beneficiary: impl Into<String>,
        amount: u64,
        start_time: u64,
    ) -> usize {
        let schedule = VestingSchedule::new(
            beneficiary,
            VestingCategory::Advisor,
            amount,
            start_time,
            ADVISOR_CLIFF_SECS,
            ADVISOR_VEST_DURATION_SECS,
        );
        self.add_schedule(schedule)
    }

    /// Create a treasury vesting schedule (no cliff).
    pub fn add_treasury_schedule(
        &mut self,
        beneficiary: impl Into<String>,
        amount: u64,
        start_time: u64,
    ) -> usize {
        let schedule = VestingSchedule::new(
            beneficiary,
            VestingCategory::Treasury,
            amount,
            start_time,
            0, // no cliff
            TREASURY_VEST_DURATION_SECS,
        );
        self.add_schedule(schedule)
    }

    /// Claim vested tokens for a beneficiary. Returns total claimed.
    pub fn claim(&mut self, beneficiary: &str, now: u64) -> Result<u64, VestingError> {
        let indices = self
            .by_beneficiary
            .get(beneficiary)
            .ok_or(VestingError::BeneficiaryNotFound)?
            .clone();

        let mut total_claimed: u64 = 0;
        for &idx in &indices {
            let schedule = &mut self.schedules[idx];
            let claimable = schedule.claimable_at(now);
            if claimable > 0 {
                schedule.claimed += claimable;
                total_claimed += claimable;
                // Check if fully vested
                if schedule.claimed >= schedule.total_amount {
                    schedule.status = ScheduleStatus::FullyVested;
                }
            }
        }
        Ok(total_claimed)
    }

    /// Revoke a vesting schedule (e.g., team member leaves).
    /// Unvested tokens return to treasury.
    pub fn revoke(&mut self, schedule_idx: usize, now: u64) -> Result<u64, VestingError> {
        let schedule = self
            .schedules
            .get_mut(schedule_idx)
            .ok_or(VestingError::ScheduleNotFound)?;

        if schedule.status != ScheduleStatus::Active {
            return Err(VestingError::AlreadyRevoked);
        }

        let vested = schedule.vested_at(now);
        let returned = schedule.total_amount.saturating_sub(vested);
        schedule.status = ScheduleStatus::Revoked { revoked_at: now };

        Ok(returned)
    }

    /// Get schedule by index.
    pub fn get_schedule(&self, idx: usize) -> Option<&VestingSchedule> {
        self.schedules.get(idx)
    }

    /// All schedules for a beneficiary.
    pub fn get_by_beneficiary(&self, beneficiary: &str) -> Vec<&VestingSchedule> {
        self.by_beneficiary
            .get(beneficiary)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|&i| self.schedules.get(i))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Total number of schedules.
    pub fn len(&self) -> usize {
        self.schedules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.schedules.is_empty()
    }

    /// Summary stats as JSON.
    pub fn summary_json(&self, now: u64) -> serde_json::Value {
        let total_allocated: u64 = self.schedules.iter().map(|s| s.total_amount).sum();
        let total_vested: u64 = self.schedules.iter().map(|s| s.vested_at(now)).sum();
        let total_claimed: u64 = self.schedules.iter().map(|s| s.claimed).sum();
        let active = self
            .schedules
            .iter()
            .filter(|s| s.status == ScheduleStatus::Active)
            .count();
        let revoked = self
            .schedules
            .iter()
            .filter(|s| matches!(s.status, ScheduleStatus::Revoked { .. }))
            .count();

        serde_json::json!({
            "total_schedules": self.schedules.len(),
            "active_schedules": active,
            "revoked_schedules": revoked,
            "total_allocated": total_allocated,
            "total_vested": total_vested,
            "total_claimed": total_claimed,
            "total_unvested": total_allocated - total_vested,
        })
    }
}

impl Default for VestingRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum VestingError {
    BeneficiaryNotFound,
    ScheduleNotFound,
    AlreadyRevoked,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const START: u64 = 1_000_000;
    const YEAR: u64 = 365 * 24 * 3600;
    const MONTH: u64 = 30 * 24 * 3600;

    #[test]
    fn test_team_schedule_cliff() {
        let s = VestingSchedule::new(
            "alice",
            VestingCategory::Team,
            1_000_000,
            START,
            TEAM_CLIFF_SECS,
            TEAM_VEST_DURATION_SECS,
        );
        // Before cliff: 0 vested
        assert_eq!(s.vested_at(START + MONTH), 0);
        assert_eq!(s.vested_at(START + 6 * MONTH), 0);
        assert_eq!(s.vested_at(START + TEAM_CLIFF_SECS - 1), 0);
    }

    #[test]
    fn test_team_schedule_after_cliff() {
        let s = VestingSchedule::new(
            "alice",
            VestingCategory::Team,
            1_000_000,
            START,
            TEAM_CLIFF_SECS,
            TEAM_VEST_DURATION_SECS,
        );
        // Just after cliff (1 year): ~25% vested
        let vested = s.vested_at(START + YEAR);
        assert!(
            vested > 240_000 && vested < 260_000,
            "vested={vested} should be ~250k"
        );
    }

    #[test]
    fn test_team_schedule_fully_vested() {
        let s = VestingSchedule::new(
            "alice",
            VestingCategory::Team,
            1_000_000,
            START,
            TEAM_CLIFF_SECS,
            TEAM_VEST_DURATION_SECS,
        );
        assert_eq!(s.vested_at(START + TEAM_VEST_DURATION_SECS), 1_000_000);
        assert_eq!(
            s.vested_at(START + TEAM_VEST_DURATION_SECS + YEAR),
            1_000_000
        );
    }

    #[test]
    fn test_advisor_schedule_cliff() {
        let s = VestingSchedule::new(
            "bob",
            VestingCategory::Advisor,
            500_000,
            START,
            ADVISOR_CLIFF_SECS,
            ADVISOR_VEST_DURATION_SECS,
        );
        assert_eq!(s.vested_at(START + ADVISOR_CLIFF_SECS - 1), 0);
        assert!(s.vested_at(START + ADVISOR_CLIFF_SECS) > 0);
    }

    #[test]
    fn test_treasury_no_cliff() {
        let s = VestingSchedule::new(
            "treasury",
            VestingCategory::Treasury,
            300_000_000,
            START,
            0,
            TREASURY_VEST_DURATION_SECS,
        );
        // Immediately some tokens vest
        assert!(s.vested_at(START + MONTH) > 0);
    }

    #[test]
    fn test_claim_updates_state() {
        let mut reg = VestingRegistry::new();
        reg.add_team_schedule("alice", 1_000_000, START);
        // After 2 years: ~50% vested
        let claimed = reg.claim("alice", START + 2 * YEAR).unwrap();
        assert!(claimed > 400_000 && claimed < 600_000, "claimed={claimed}");
        // Claim again at same time: 0 more
        let again = reg.claim("alice", START + 2 * YEAR).unwrap();
        assert_eq!(again, 0);
    }

    #[test]
    fn test_claim_unknown_beneficiary() {
        let mut reg = VestingRegistry::new();
        assert_eq!(
            reg.claim("nobody", START),
            Err(VestingError::BeneficiaryNotFound)
        );
    }

    #[test]
    fn test_revoke_returns_unvested() {
        let mut reg = VestingRegistry::new();
        let idx = reg.add_team_schedule("alice", 1_000_000, START);
        // Revoke after 2 years: ~50% vested → ~500k returned
        let returned = reg.revoke(idx, START + 2 * YEAR).unwrap();
        assert!(
            returned > 400_000 && returned < 600_000,
            "returned={returned}"
        );
        // Schedule is now revoked
        assert!(matches!(
            reg.get_schedule(idx).unwrap().status,
            ScheduleStatus::Revoked { .. }
        ));
    }

    #[test]
    fn test_revoke_before_cliff_returns_all() {
        let mut reg = VestingRegistry::new();
        let idx = reg.add_team_schedule("alice", 1_000_000, START);
        let returned = reg.revoke(idx, START + MONTH).unwrap(); // before cliff
        assert_eq!(returned, 1_000_000);
    }

    #[test]
    fn test_double_revoke_fails() {
        let mut reg = VestingRegistry::new();
        let idx = reg.add_team_schedule("alice", 1_000_000, START);
        reg.revoke(idx, START + 2 * YEAR).unwrap();
        assert_eq!(
            reg.revoke(idx, START + 3 * YEAR),
            Err(VestingError::AlreadyRevoked)
        );
    }

    #[test]
    fn test_revoked_schedule_caps_vesting() {
        let mut reg = VestingRegistry::new();
        let idx = reg.add_team_schedule("alice", 1_000_000, START);
        let revoke_time = START + 2 * YEAR;
        reg.revoke(idx, revoke_time).unwrap();
        let schedule = reg.get_schedule(idx).unwrap();
        let vested_at_revoke = schedule.vested_at(revoke_time);
        // Even far in the future, vesting is capped at revocation time
        let vested_later = schedule.vested_at(START + 10 * YEAR);
        assert_eq!(vested_at_revoke, vested_later);
    }

    #[test]
    fn test_multiple_schedules_per_beneficiary() {
        let mut reg = VestingRegistry::new();
        reg.add_team_schedule("alice", 500_000, START);
        reg.add_advisor_schedule("alice", 200_000, START);
        let schedules = reg.get_by_beneficiary("alice");
        assert_eq!(schedules.len(), 2);
    }

    #[test]
    fn test_fully_vested_status() {
        let mut reg = VestingRegistry::new();
        reg.add_team_schedule("alice", 1_000_000, START);
        let _ = reg.claim("alice", START + TEAM_VEST_DURATION_SECS + 1);
        let schedule = reg.get_schedule(0).unwrap();
        assert_eq!(schedule.status, ScheduleStatus::FullyVested);
        assert_eq!(schedule.claimed, 1_000_000);
    }

    #[test]
    fn test_summary_json() {
        let mut reg = VestingRegistry::new();
        reg.add_team_schedule("alice", 1_000_000, START);
        reg.add_treasury_schedule("dao", 300_000_000, START);
        let j = reg.summary_json(START + YEAR);
        assert_eq!(j["total_schedules"], 2);
        assert_eq!(j["active_schedules"], 2);
        assert!(j["total_vested"].as_u64().unwrap() > 0);
    }

    #[test]
    fn test_registry_len() {
        let mut reg = VestingRegistry::new();
        assert!(reg.is_empty());
        reg.add_team_schedule("a", 100, START);
        reg.add_advisor_schedule("b", 200, START);
        assert_eq!(reg.len(), 2);
    }
}
