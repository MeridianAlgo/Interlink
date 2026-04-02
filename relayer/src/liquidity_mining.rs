/// Liquidity mining incentive program for InterLink (Phase 9)
///
/// Distributes $INTERLINK token rewards to LPs who provide bridge liquidity.
/// Epoch-based reward schedule with boost multipliers, vesting, and anti-gaming.
///
/// Program parameters:
///   - Total budget:     10,000,000 $INTERLINK over 26 epochs (6 months)
///   - Epoch length:     7 days
///   - Early-bird boost: 2x rewards in epochs 1-4
///   - Loyalty boost:    1.5x for LPs who stay ≥4 consecutive epochs
///   - Vesting:          25% immediate, 75% linear over 90 days
///   - Anti-gaming:      Minimum 24h deposit before earning rewards
///
/// Comparison:
///   Stargate:  STG emissions to LPs, ~$5M/month, no vesting
///   Across:    ACX rewards, weekly distribution, no boost
///   Uniswap:   UNI mining ended, was purely proportional
///   InterLink: epoch-based with boost + vesting + anti-gaming (more sustainable)
use std::collections::HashMap;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Total reward budget in token units (10M $INTERLINK).
pub const TOTAL_REWARD_BUDGET: u64 = 10_000_000;
/// Number of epochs in the program (26 weeks = 6 months).
pub const TOTAL_EPOCHS: u32 = 26;
/// Epoch duration in seconds (7 days).
pub const EPOCH_DURATION_SECS: u64 = 7 * 24 * 3600;
/// Early-bird boost multiplier (2x) applies to first N epochs.
pub const EARLY_BIRD_EPOCHS: u32 = 4;
/// Early-bird boost in basis points (20000 = 2.0x).
pub const EARLY_BIRD_BOOST_BPS: u32 = 20_000;
/// Loyalty boost: consecutive epochs required to qualify.
pub const LOYALTY_THRESHOLD_EPOCHS: u32 = 4;
/// Loyalty boost in basis points (15000 = 1.5x).
pub const LOYALTY_BOOST_BPS: u32 = 15_000;
/// Anti-gaming: minimum deposit duration before earning (24 hours).
pub const MIN_DEPOSIT_DURATION_SECS: u64 = 24 * 3600;
/// Vesting: immediate release percentage (25%).
pub const IMMEDIATE_VEST_BPS: u32 = 2_500;
/// Vesting: linear release period in seconds (90 days).
pub const LINEAR_VEST_DURATION_SECS: u64 = 90 * 24 * 3600;
/// Basis point denominator.
const BPS: u64 = 10_000;

// ─── Types ──────────────────────────────────────────────────────────────────

/// A liquidity provider's position.
#[derive(Debug, Clone)]
pub struct LpPosition {
    /// LP identifier (address).
    pub lp_id: String,
    /// Amount of liquidity deposited (in token units).
    pub liquidity: u128,
    /// Timestamp when deposit was made.
    pub deposit_timestamp: u64,
    /// Number of consecutive epochs this LP has participated.
    pub consecutive_epochs: u32,
    /// Total rewards earned (before vesting).
    pub total_rewards_earned: u64,
    /// Rewards already released (immediate + vested).
    pub rewards_released: u64,
    /// Rewards pending vesting.
    pub rewards_vesting: Vec<VestingSchedule>,
}

/// A vesting schedule for a reward tranche.
#[derive(Debug, Clone)]
pub struct VestingSchedule {
    /// Total amount in this tranche.
    pub total: u64,
    /// Amount already claimed.
    pub claimed: u64,
    /// Vesting start timestamp.
    pub start_timestamp: u64,
    /// Vesting end timestamp.
    pub end_timestamp: u64,
}

impl VestingSchedule {
    /// Compute claimable amount at a given timestamp.
    pub fn claimable_at(&self, now: u64) -> u64 {
        if now >= self.end_timestamp {
            self.total.saturating_sub(self.claimed)
        } else if now <= self.start_timestamp {
            0
        } else {
            let elapsed = now - self.start_timestamp;
            let duration = self.end_timestamp - self.start_timestamp;
            let vested = (self.total as u128 * elapsed as u128 / duration as u128) as u64;
            vested.saturating_sub(self.claimed)
        }
    }
}

/// Summary of one epoch's reward distribution.
#[derive(Debug, Clone)]
pub struct EpochRewardSummary {
    pub epoch: u32,
    pub total_distributed: u64,
    pub eligible_lps: usize,
    pub total_liquidity: u128,
    pub boost_active: bool,
    pub per_lp: Vec<LpReward>,
}

/// Reward detail for a single LP in an epoch.
#[derive(Debug, Clone)]
pub struct LpReward {
    pub lp_id: String,
    pub base_reward: u64,
    pub boost_multiplier_bps: u32,
    pub final_reward: u64,
    pub immediate_release: u64,
    pub vesting_amount: u64,
}

// ─── Mining Program ─────────────────────────────────────────────────────────

pub struct LiquidityMiningProgram {
    /// All LP positions.
    positions: HashMap<String, LpPosition>,
    /// Current epoch number (0-indexed).
    current_epoch: u32,
    /// Timestamp when the program started.
    program_start: u64,
    /// Total rewards distributed so far.
    total_distributed: u64,
    /// Per-epoch reward budget (decreasing schedule).
    epoch_budgets: Vec<u64>,
}

impl LiquidityMiningProgram {
    /// Create a new mining program starting at `start_timestamp`.
    pub fn new(start_timestamp: u64) -> Self {
        let epoch_budgets = compute_epoch_budgets();
        LiquidityMiningProgram {
            positions: HashMap::new(),
            current_epoch: 0,
            program_start: start_timestamp,
            total_distributed: 0,
            epoch_budgets,
        }
    }

    /// Deposit liquidity. Returns the LP position.
    pub fn deposit(&mut self, lp_id: impl Into<String>, liquidity: u128, now: u64) -> &LpPosition {
        let id = lp_id.into();
        let pos = self
            .positions
            .entry(id.clone())
            .or_insert_with(|| LpPosition {
                lp_id: id,
                liquidity: 0,
                deposit_timestamp: now,
                consecutive_epochs: 0,
                total_rewards_earned: 0,
                rewards_released: 0,
                rewards_vesting: Vec::new(),
            });
        pos.liquidity += liquidity;
        // Reset deposit timestamp if adding to existing position
        if pos.liquidity == liquidity {
            pos.deposit_timestamp = now;
        }
        pos
    }

    /// Withdraw liquidity. Returns remaining amount.
    pub fn withdraw(&mut self, lp_id: &str, amount: u128) -> Result<u128, MiningError> {
        let pos = self
            .positions
            .get_mut(lp_id)
            .ok_or(MiningError::LpNotFound)?;
        if amount > pos.liquidity {
            return Err(MiningError::InsufficientLiquidity);
        }
        pos.liquidity -= amount;
        // Reset consecutive epochs on full withdrawal
        if pos.liquidity == 0 {
            pos.consecutive_epochs = 0;
        }
        Ok(pos.liquidity)
    }

    /// Distribute rewards for the current epoch.
    pub fn distribute_epoch(&mut self, now: u64) -> Result<EpochRewardSummary, MiningError> {
        if self.current_epoch >= TOTAL_EPOCHS {
            return Err(MiningError::ProgramEnded);
        }

        let epoch = self.current_epoch;
        let budget = self.epoch_budgets.get(epoch as usize).copied().unwrap_or(0);
        let epoch_start = self.program_start + (epoch as u64 * EPOCH_DURATION_SECS);

        // Filter eligible LPs (minimum deposit duration + non-zero liquidity)
        let eligible: Vec<(String, u128, u32)> = self
            .positions
            .iter()
            .filter(|(_, p)| {
                p.liquidity > 0
                    && now.saturating_sub(p.deposit_timestamp) >= MIN_DEPOSIT_DURATION_SECS
            })
            .map(|(id, p)| (id.clone(), p.liquidity, p.consecutive_epochs))
            .collect();

        if eligible.is_empty() {
            self.current_epoch += 1;
            return Ok(EpochRewardSummary {
                epoch,
                total_distributed: 0,
                eligible_lps: 0,
                total_liquidity: 0,
                boost_active: epoch < EARLY_BIRD_EPOCHS,
                per_lp: Vec::new(),
            });
        }

        // Compute weighted shares
        let total_weighted: u128 = eligible
            .iter()
            .map(|(_, liq, consec)| {
                let boost = compute_boost_bps(epoch, *consec);
                (*liq as u128) * (boost as u128) / (BPS as u128)
            })
            .sum();

        let mut per_lp = Vec::new();
        let mut actual_distributed: u64 = 0;

        for (id, liq, consec) in &eligible {
            let boost = compute_boost_bps(epoch, *consec);
            let weighted = (*liq as u128) * (boost as u128) / (BPS as u128);
            let base_reward = if total_weighted > 0 {
                (budget as u128 * (*liq as u128) / total_weighted.max(1)) as u64
            } else {
                0
            };
            let final_reward = if total_weighted > 0 {
                (budget as u128 * weighted / total_weighted) as u64
            } else {
                0
            };

            // Split into immediate + vesting
            let immediate =
                (final_reward as u128 * IMMEDIATE_VEST_BPS as u128 / BPS as u128) as u64;
            let vesting_amount = final_reward.saturating_sub(immediate);

            per_lp.push(LpReward {
                lp_id: id.clone(),
                base_reward,
                boost_multiplier_bps: boost,
                final_reward,
                immediate_release: immediate,
                vesting_amount,
            });

            // Update LP position
            if let Some(pos) = self.positions.get_mut(id) {
                pos.total_rewards_earned += final_reward;
                pos.rewards_released += immediate;
                pos.consecutive_epochs += 1;
                if vesting_amount > 0 {
                    pos.rewards_vesting.push(VestingSchedule {
                        total: vesting_amount,
                        claimed: 0,
                        start_timestamp: now,
                        end_timestamp: now + LINEAR_VEST_DURATION_SECS,
                    });
                }
            }

            actual_distributed += final_reward;
        }

        let total_liquidity: u128 = eligible.iter().map(|(_, l, _)| *l as u128).sum();
        self.total_distributed += actual_distributed;
        self.current_epoch += 1;

        Ok(EpochRewardSummary {
            epoch,
            total_distributed: actual_distributed,
            eligible_lps: eligible.len(),
            total_liquidity,
            boost_active: epoch < EARLY_BIRD_EPOCHS,
            per_lp,
        })
    }

    /// Claim vested rewards for an LP. Returns amount claimed.
    pub fn claim_vested(&mut self, lp_id: &str, now: u64) -> Result<u64, MiningError> {
        let pos = self
            .positions
            .get_mut(lp_id)
            .ok_or(MiningError::LpNotFound)?;
        let mut total_claimed: u64 = 0;
        for schedule in &mut pos.rewards_vesting {
            let claimable = schedule.claimable_at(now);
            if claimable > 0 {
                schedule.claimed += claimable;
                total_claimed += claimable;
            }
        }
        pos.rewards_released += total_claimed;
        Ok(total_claimed)
    }

    /// Get LP position details.
    pub fn get_position(&self, lp_id: &str) -> Option<&LpPosition> {
        self.positions.get(lp_id)
    }

    /// Current epoch number.
    pub fn current_epoch(&self) -> u32 {
        self.current_epoch
    }

    /// Total rewards distributed so far.
    pub fn total_distributed(&self) -> u64 {
        self.total_distributed
    }

    /// Remaining reward budget.
    pub fn remaining_budget(&self) -> u64 {
        TOTAL_REWARD_BUDGET.saturating_sub(self.total_distributed)
    }

    /// Program stats as JSON.
    pub fn stats_json(&self) -> serde_json::Value {
        let active_lps = self.positions.values().filter(|p| p.liquidity > 0).count();
        let total_liquidity: u128 = self.positions.values().map(|p| p.liquidity).sum();
        serde_json::json!({
            "current_epoch": self.current_epoch,
            "total_epochs": TOTAL_EPOCHS,
            "total_distributed": self.total_distributed,
            "remaining_budget": self.remaining_budget(),
            "active_lps": active_lps,
            "total_liquidity": total_liquidity.to_string(),
            "program_ended": self.current_epoch >= TOTAL_EPOCHS,
        })
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Compute per-epoch budgets with front-loaded schedule.
/// Early epochs get more rewards to bootstrap liquidity.
fn compute_epoch_budgets() -> Vec<u64> {
    // Front-loaded: first 4 epochs get 2x weight, remaining get 1x
    let early_weight: u64 = EARLY_BIRD_EPOCHS as u64 * 2;
    let late_weight: u64 = (TOTAL_EPOCHS - EARLY_BIRD_EPOCHS) as u64;
    let total_weight = early_weight + late_weight;

    let mut budgets = Vec::with_capacity(TOTAL_EPOCHS as usize);
    for e in 0..TOTAL_EPOCHS {
        let weight = if e < EARLY_BIRD_EPOCHS { 2 } else { 1 };
        let budget = TOTAL_REWARD_BUDGET as u128 * weight as u128 / total_weight as u128;
        budgets.push(budget as u64);
    }
    budgets
}

/// Compute effective boost in basis points for an LP.
fn compute_boost_bps(epoch: u32, consecutive_epochs: u32) -> u32 {
    let mut boost = BPS as u32; // 1.0x base

    // Early-bird boost
    if epoch < EARLY_BIRD_EPOCHS {
        boost = boost * EARLY_BIRD_BOOST_BPS / (BPS as u32);
    }

    // Loyalty boost
    if consecutive_epochs >= LOYALTY_THRESHOLD_EPOCHS {
        boost = boost * LOYALTY_BOOST_BPS / (BPS as u32);
    }

    boost
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum MiningError {
    LpNotFound,
    InsufficientLiquidity,
    ProgramEnded,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const START: u64 = 1_000_000;
    const DAY: u64 = 86_400;

    #[test]
    fn test_deposit_creates_position() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        let pos = prog.get_position("alice").unwrap();
        assert_eq!(pos.liquidity, 1_000_000);
        assert_eq!(pos.consecutive_epochs, 0);
    }

    #[test]
    fn test_deposit_adds_to_existing() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 500, START);
        prog.deposit("alice", 300, START + 100);
        assert_eq!(prog.get_position("alice").unwrap().liquidity, 800);
    }

    #[test]
    fn test_withdraw_reduces_liquidity() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1000, START);
        let remaining = prog.withdraw("alice", 400).unwrap();
        assert_eq!(remaining, 600);
    }

    #[test]
    fn test_withdraw_insufficient() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 100, START);
        assert_eq!(
            prog.withdraw("alice", 200),
            Err(MiningError::InsufficientLiquidity)
        );
    }

    #[test]
    fn test_withdraw_unknown_lp() {
        let mut prog = LiquidityMiningProgram::new(START);
        assert_eq!(prog.withdraw("unknown", 1), Err(MiningError::LpNotFound));
    }

    #[test]
    fn test_anti_gaming_minimum_deposit_duration() {
        let mut prog = LiquidityMiningProgram::new(START);
        // Deposit just now — not eligible yet (< 24h)
        prog.deposit("alice", 1_000_000, START);
        let summary = prog.distribute_epoch(START + 3600).unwrap(); // 1 hour later
        assert_eq!(summary.eligible_lps, 0);
        assert_eq!(summary.total_distributed, 0);
    }

    #[test]
    fn test_eligible_after_minimum_duration() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        let now = START + EPOCH_DURATION_SECS; // 7 days later (> 24h)
        let summary = prog.distribute_epoch(now).unwrap();
        assert_eq!(summary.eligible_lps, 1);
        assert!(summary.total_distributed > 0);
    }

    #[test]
    fn test_early_bird_boost_active() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        let now = START + 2 * DAY;
        let summary = prog.distribute_epoch(now).unwrap();
        assert!(summary.boost_active);
        // Early bird: epoch 0 should get 2x weight budget
        assert!(summary.per_lp[0].boost_multiplier_bps >= 20_000);
    }

    #[test]
    fn test_no_early_bird_after_threshold() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        // Advance past early bird epochs
        for i in 0..EARLY_BIRD_EPOCHS {
            let now = START + ((i + 1) as u64 * EPOCH_DURATION_SECS);
            let _ = prog.distribute_epoch(now);
        }
        let now = START + ((EARLY_BIRD_EPOCHS + 1) as u64 * EPOCH_DURATION_SECS);
        let summary = prog.distribute_epoch(now).unwrap();
        assert!(!summary.boost_active);
    }

    #[test]
    fn test_loyalty_boost_after_consecutive_epochs() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        // Run 4 epochs to build consecutive count
        for i in 0..LOYALTY_THRESHOLD_EPOCHS {
            let now = START + ((i + 1) as u64 * EPOCH_DURATION_SECS);
            let _ = prog.distribute_epoch(now);
        }
        // 5th epoch: loyalty boost should kick in
        let pos = prog.get_position("alice").unwrap();
        assert!(pos.consecutive_epochs >= LOYALTY_THRESHOLD_EPOCHS);
    }

    #[test]
    fn test_vesting_schedule_25_75_split() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        let now = START + 2 * DAY;
        let summary = prog.distribute_epoch(now).unwrap();
        let reward = &summary.per_lp[0];
        // 25% immediate
        let expected_immediate = reward.final_reward * IMMEDIATE_VEST_BPS as u64 / BPS;
        assert_eq!(reward.immediate_release, expected_immediate);
        // 75% vesting
        assert_eq!(
            reward.vesting_amount,
            reward.final_reward - expected_immediate
        );
    }

    #[test]
    fn test_claim_vested_partial() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        let dist_time = START + 2 * DAY;
        let _ = prog.distribute_epoch(dist_time);

        // Claim at 50% through vesting period
        let half_vest = dist_time + LINEAR_VEST_DURATION_SECS / 2;
        let claimed = prog.claim_vested("alice", half_vest).unwrap();
        assert!(claimed > 0);

        // Claim again at end
        let end = dist_time + LINEAR_VEST_DURATION_SECS + 1;
        let final_claim = prog.claim_vested("alice", end).unwrap();
        assert!(final_claim > 0);
    }

    #[test]
    fn test_claim_vested_nothing_before_start() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        // No distribution yet, so no vesting
        let claimed = prog.claim_vested("alice", START).unwrap();
        assert_eq!(claimed, 0);
    }

    #[test]
    fn test_program_ends_after_total_epochs() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        for i in 0..TOTAL_EPOCHS {
            let now = START + ((i + 1) as u64 * EPOCH_DURATION_SECS);
            let _ = prog.distribute_epoch(now);
        }
        // Epoch 26 should fail
        let result = prog.distribute_epoch(START + 27 * EPOCH_DURATION_SECS);
        assert_eq!(result.unwrap_err(), MiningError::ProgramEnded);
    }

    #[test]
    fn test_total_budget_conserved() {
        let budgets = compute_epoch_budgets();
        let sum: u64 = budgets.iter().sum();
        // Allow small rounding error (< 100 tokens out of 10M)
        assert!(
            sum <= TOTAL_REWARD_BUDGET && sum > TOTAL_REWARD_BUDGET - 100,
            "Budget sum={sum} should be close to {TOTAL_REWARD_BUDGET}"
        );
    }

    #[test]
    fn test_multiple_lps_proportional() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 3_000_000, START);
        prog.deposit("bob", 1_000_000, START);
        let now = START + 2 * DAY;
        let summary = prog.distribute_epoch(now).unwrap();
        assert_eq!(summary.eligible_lps, 2);
        // Alice has 3x Bob's liquidity, should get ~3x rewards
        let alice_reward = summary.per_lp.iter().find(|r| r.lp_id == "alice").unwrap();
        let bob_reward = summary.per_lp.iter().find(|r| r.lp_id == "bob").unwrap();
        // Allow some rounding tolerance
        let ratio = alice_reward.final_reward as f64 / bob_reward.final_reward.max(1) as f64;
        assert!(ratio > 2.5 && ratio < 3.5, "ratio={ratio} should be ~3.0");
    }

    #[test]
    fn test_full_withdrawal_resets_consecutive() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1000, START);
        let _ = prog.distribute_epoch(START + 2 * DAY);
        assert_eq!(prog.get_position("alice").unwrap().consecutive_epochs, 1);
        prog.withdraw("alice", 1000).unwrap();
        assert_eq!(prog.get_position("alice").unwrap().consecutive_epochs, 0);
    }

    #[test]
    fn test_stats_json() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 5000, START);
        let j = prog.stats_json();
        assert_eq!(j["current_epoch"], 0);
        assert_eq!(j["total_epochs"], TOTAL_EPOCHS);
        assert_eq!(j["active_lps"], 1);
        assert_eq!(j["program_ended"], false);
    }

    #[test]
    fn test_remaining_budget_decreases() {
        let mut prog = LiquidityMiningProgram::new(START);
        prog.deposit("alice", 1_000_000, START);
        let before = prog.remaining_budget();
        let _ = prog.distribute_epoch(START + 2 * DAY);
        let after = prog.remaining_budget();
        assert!(after < before);
    }

    #[test]
    fn test_empty_epoch_no_distribution() {
        let mut prog = LiquidityMiningProgram::new(START);
        // No LPs deposited
        let summary = prog.distribute_epoch(START + 2 * DAY).unwrap();
        assert_eq!(summary.eligible_lps, 0);
        assert_eq!(summary.total_distributed, 0);
    }
}
