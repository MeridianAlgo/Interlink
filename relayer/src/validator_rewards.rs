/// Validator incentive program for InterLink (Phase 9)
///
/// Distributes bridge fee revenue to validators proportional to their uptime
/// and stake weight. Includes penalty deductions for downtime.
///
/// Revenue split:
///   - 10% of bridge fees → validator reward pool
///   - Distribution: (stake_weight × uptime_factor) / total_eligible
///
/// Comparison:
///   Wormhole:  guardians earn from per-VAA fees (opaque economics)
///   Stargate:  STG stakers earn from cross-chain fees (~8-12% APY)
///   Across:    UMA voters earn from dispute resolution fees
///   InterLink: transparent 10% fee-share + uptime-weighted distribution

use std::collections::HashMap;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Fraction of bridge fees allocated to validator rewards (basis points).
pub const VALIDATOR_SHARE_BPS: u32 = 1000; // 10%
/// Minimum uptime to qualify for rewards (basis points of expected uptime).
pub const MIN_UPTIME_BPS: u32 = 9000; // 90%
/// Bonus multiplier for 100% uptime validators (basis points, 100% = 10000).
pub const PERFECT_UPTIME_BONUS_BPS: u32 = 500; // +5% bonus
/// Epoch duration in seconds (1 day).
pub const EPOCH_DURATION_SECS: u64 = 86_400;

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ValidatorRecord {
    pub id: String,
    pub stake: u64,
    /// Expected heartbeats in the epoch
    pub expected_heartbeats: u64,
    /// Actual heartbeats received
    pub actual_heartbeats: u64,
    /// Cumulative rewards earned (all time)
    pub total_rewards: u64,
    /// Rewards earned in current epoch
    pub epoch_rewards: u64,
    /// Whether currently active
    pub active: bool,
}

impl ValidatorRecord {
    pub fn new(id: impl Into<String>, stake: u64) -> Self {
        ValidatorRecord {
            id: id.into(),
            stake,
            expected_heartbeats: 0,
            actual_heartbeats: 0,
            total_rewards: 0,
            epoch_rewards: 0,
            active: true,
        }
    }

    /// Uptime as basis points (0-10000).
    pub fn uptime_bps(&self) -> u32 {
        if self.expected_heartbeats == 0 {
            return 10_000;
        }
        let ratio = self.actual_heartbeats as u128 * 10_000 / self.expected_heartbeats as u128;
        ratio.min(10_000) as u32
    }

    /// Whether this validator qualifies for rewards.
    pub fn eligible(&self) -> bool {
        self.active && self.uptime_bps() >= MIN_UPTIME_BPS
    }

    /// Effective weight = stake × uptime_factor. Bonus for perfect uptime.
    pub fn effective_weight(&self) -> u128 {
        if !self.eligible() {
            return 0;
        }
        let uptime = self.uptime_bps() as u128;
        let bonus = if self.uptime_bps() == 10_000 {
            PERFECT_UPTIME_BONUS_BPS as u128
        } else {
            0
        };
        self.stake as u128 * (uptime + bonus) / 10_000
    }
}

#[derive(Debug, Clone)]
pub struct EpochSummary {
    pub epoch: u64,
    pub total_fees_collected: u64,
    pub validator_pool: u64,
    pub distributions: Vec<(String, u64)>,
    pub ineligible: Vec<(String, u32)>, // (id, uptime_bps)
}

// ─── Reward Distributor ──────────────────────────────────────────────────────

pub struct RewardDistributor {
    pub validators: HashMap<String, ValidatorRecord>,
    pub current_epoch: u64,
    pub total_distributed: u64,
}

impl RewardDistributor {
    pub fn new() -> Self {
        RewardDistributor {
            validators: HashMap::new(),
            current_epoch: 0,
            total_distributed: 0,
        }
    }

    /// Register or update a validator's stake.
    pub fn register(&mut self, id: impl Into<String>, stake: u64) {
        let id = id.into();
        self.validators
            .entry(id.clone())
            .and_modify(|v| v.stake = stake)
            .or_insert_with(|| ValidatorRecord::new(id, stake));
    }

    /// Record a heartbeat for a validator.
    pub fn record_heartbeat(&mut self, validator_id: &str) {
        if let Some(v) = self.validators.get_mut(validator_id) {
            v.actual_heartbeats += 1;
        }
    }

    /// Set expected heartbeats for all validators in this epoch.
    pub fn set_expected_heartbeats(&mut self, count: u64) {
        for v in self.validators.values_mut() {
            v.expected_heartbeats = count;
        }
    }

    /// Deactivate a validator (e.g., slashed or voluntarily exited).
    pub fn deactivate(&mut self, validator_id: &str) -> bool {
        if let Some(v) = self.validators.get_mut(validator_id) {
            v.active = false;
            true
        } else {
            false
        }
    }

    /// Distribute rewards for the epoch based on bridge fees collected.
    ///
    /// Returns an `EpochSummary` showing who got what.
    pub fn distribute_epoch(
        &mut self,
        total_fees_collected: u64,
    ) -> EpochSummary {
        let validator_pool =
            (total_fees_collected as u128 * VALIDATOR_SHARE_BPS as u128 / 10_000) as u64;

        let eligible: Vec<(String, u128)> = self
            .validators
            .iter()
            .filter(|(_, v)| v.eligible())
            .map(|(id, v)| (id.clone(), v.effective_weight()))
            .collect();

        let total_weight: u128 = eligible.iter().map(|(_, w)| w).sum();

        let mut distributions = Vec::new();
        let mut distributed = 0u64;

        if total_weight > 0 {
            for (id, weight) in &eligible {
                let reward = (validator_pool as u128 * weight / total_weight) as u64;
                if let Some(v) = self.validators.get_mut(id) {
                    v.epoch_rewards = reward;
                    v.total_rewards += reward;
                }
                distributed += reward;
                distributions.push((id.clone(), reward));
            }
        }

        let ineligible: Vec<(String, u32)> = self
            .validators
            .iter()
            .filter(|(_, v)| !v.eligible())
            .map(|(id, v)| (id.clone(), v.uptime_bps()))
            .collect();

        self.total_distributed += distributed;
        self.current_epoch += 1;

        // Reset heartbeats for next epoch
        for v in self.validators.values_mut() {
            v.expected_heartbeats = 0;
            v.actual_heartbeats = 0;
            v.epoch_rewards = 0;
        }

        EpochSummary {
            epoch: self.current_epoch - 1,
            total_fees_collected,
            validator_pool,
            distributions,
            ineligible,
        }
    }

    /// Number of active validators.
    pub fn active_count(&self) -> usize {
        self.validators.values().filter(|v| v.active).count()
    }

    /// Total staked across all active validators.
    pub fn total_stake(&self) -> u64 {
        self.validators
            .values()
            .filter(|v| v.active)
            .map(|v| v.stake)
            .sum()
    }
}

impl Default for RewardDistributor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_3_validators() -> RewardDistributor {
        let mut rd = RewardDistributor::new();
        rd.register("v0", 100_000);
        rd.register("v1", 200_000);
        rd.register("v2", 300_000);
        rd.set_expected_heartbeats(100);
        // v0: 100% uptime
        for _ in 0..100 { rd.record_heartbeat("v0"); }
        // v1: 95% uptime
        for _ in 0..95 { rd.record_heartbeat("v1"); }
        // v2: 80% uptime (below MIN_UPTIME_BPS=90%)
        for _ in 0..80 { rd.record_heartbeat("v2"); }
        rd
    }

    #[test]
    fn test_uptime_calculation() {
        let rd = setup_3_validators();
        assert_eq!(rd.validators["v0"].uptime_bps(), 10_000); // 100%
        assert_eq!(rd.validators["v1"].uptime_bps(), 9_500);  // 95%
        assert_eq!(rd.validators["v2"].uptime_bps(), 8_000);  // 80%
    }

    #[test]
    fn test_eligibility() {
        let rd = setup_3_validators();
        assert!(rd.validators["v0"].eligible());
        assert!(rd.validators["v1"].eligible());
        assert!(!rd.validators["v2"].eligible()); // below 90%
    }

    #[test]
    fn test_effective_weight_includes_perfect_uptime_bonus() {
        let rd = setup_3_validators();
        let w0 = rd.validators["v0"].effective_weight();
        // 100_000 * (10000 + 500) / 10000 = 105_000
        assert_eq!(w0, 105_000);
        let w1 = rd.validators["v1"].effective_weight();
        // 200_000 * (9500 + 0) / 10000 = 190_000
        assert_eq!(w1, 190_000);
        // v2 is ineligible
        assert_eq!(rd.validators["v2"].effective_weight(), 0);
    }

    #[test]
    fn test_distribution_proportional() {
        let mut rd = setup_3_validators();
        let summary = rd.distribute_epoch(1_000_000); // $10k in fees

        // Validator pool = 10% of 1M = 100_000
        assert_eq!(summary.validator_pool, 100_000);

        // Only v0 and v1 eligible
        assert_eq!(summary.distributions.len(), 2);
        assert_eq!(summary.ineligible.len(), 1);
        assert_eq!(summary.ineligible[0].0, "v2");

        let total_reward: u64 = summary.distributions.iter().map(|(_, r)| r).sum();
        // Rounding may lose a few cents — should be within 1 of pool
        assert!(total_reward <= summary.validator_pool);
        assert!(summary.validator_pool - total_reward <= 1);
    }

    #[test]
    fn test_higher_stake_gets_more() {
        let mut rd = setup_3_validators();
        let summary = rd.distribute_epoch(1_000_000);
        let reward_v0 = summary.distributions.iter().find(|(id, _)| id == "v0").unwrap().1;
        let reward_v1 = summary.distributions.iter().find(|(id, _)| id == "v1").unwrap().1;
        // v1 has 2× the stake of v0 but 95% vs 100% uptime
        // v0 weight: 105_000, v1 weight: 190_000
        assert!(reward_v1 > reward_v0, "higher stake should earn more");
    }

    #[test]
    fn test_deactivated_validator_ineligible() {
        let mut rd = setup_3_validators();
        rd.deactivate("v0");
        let summary = rd.distribute_epoch(1_000_000);
        // Only v1 eligible (v0 deactivated, v2 low uptime)
        assert_eq!(summary.distributions.len(), 1);
        assert_eq!(summary.distributions[0].0, "v1");
    }

    #[test]
    fn test_zero_fees_zero_rewards() {
        let mut rd = setup_3_validators();
        let summary = rd.distribute_epoch(0);
        assert_eq!(summary.validator_pool, 0);
        for (_, reward) in &summary.distributions {
            assert_eq!(*reward, 0);
        }
    }

    #[test]
    fn test_epoch_counter_increments() {
        let mut rd = setup_3_validators();
        let s1 = rd.distribute_epoch(100);
        assert_eq!(s1.epoch, 0);
        rd.set_expected_heartbeats(50);
        for _ in 0..50 { rd.record_heartbeat("v0"); }
        let s2 = rd.distribute_epoch(200);
        assert_eq!(s2.epoch, 1);
    }

    #[test]
    fn test_heartbeats_reset_after_epoch() {
        let mut rd = setup_3_validators();
        rd.distribute_epoch(100);
        // After distribution, heartbeats should be reset
        assert_eq!(rd.validators["v0"].expected_heartbeats, 0);
        assert_eq!(rd.validators["v0"].actual_heartbeats, 0);
    }

    #[test]
    fn test_cumulative_total_rewards() {
        let mut rd = RewardDistributor::new();
        rd.register("v0", 100_000);
        // Epoch 1
        rd.set_expected_heartbeats(10);
        for _ in 0..10 { rd.record_heartbeat("v0"); }
        rd.distribute_epoch(100_000);
        let after_e1 = rd.validators["v0"].total_rewards;
        // Epoch 2
        rd.set_expected_heartbeats(10);
        for _ in 0..10 { rd.record_heartbeat("v0"); }
        rd.distribute_epoch(200_000);
        let after_e2 = rd.validators["v0"].total_rewards;
        assert!(after_e2 > after_e1, "cumulative rewards must grow");
    }

    #[test]
    fn test_total_stake_and_active_count() {
        let mut rd = RewardDistributor::new();
        rd.register("v0", 100);
        rd.register("v1", 200);
        rd.register("v2", 300);
        assert_eq!(rd.active_count(), 3);
        assert_eq!(rd.total_stake(), 600);
        rd.deactivate("v1");
        assert_eq!(rd.active_count(), 2);
        assert_eq!(rd.total_stake(), 400);
    }
}
