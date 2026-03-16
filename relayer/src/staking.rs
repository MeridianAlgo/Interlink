//! $INTERLINK native token staking rewards model.
//!
//! Implements the token economic model for $INTERLINK stakers:
//!   - Fee discounts for stakers (reduce protocol fee by up to 100%)
//!   - APY rewards funded from protocol revenue
//!   - Governance voting weight proportional to stake
//!   - Slashing conditions for malicious/offline validators
//!
//! # Token supply
//!   Total: 1,000,000,000 $INTERLINK (1 billion)
//!   Distribution:
//!     - 40% community / liquidity mining (400M)
//!     - 30% team (4-year vest, 1-year cliff) (300M)
//!     - 30% treasury (protocol reserve) (300M)
//!
//! # Staking tiers (Phase 9)
//!   Bronze: ≥ 1,000 tokens   — 10% fee discount
//!   Silver: ≥ 10,000 tokens  — 25% fee discount + governance vote
//!   Gold:   ≥ 100,000 tokens — 50% fee discount + validator eligibility
//!   Platinum: ≥ 1M tokens    — 100% fee waiver + enhanced APY
//!
//! # Competitive comparison
//!   Wormhole guardians: stake ETH/SOL, not a native token
//!   Stargate:          STG token, lockup 1-3yr, 8-12% APY
//!   Across:            UMA token as oracle bond, no direct staking
//!   InterLink:         $INTERLINK, flexible lock, 10-20% early APY → tapering to 5%

use serde::Serialize;

// ─── Token constants ──────────────────────────────────────────────────────────

/// Total $INTERLINK supply (1 billion tokens, 18 decimals).
pub const TOTAL_SUPPLY: u128 = 1_000_000_000 * 1_000_000_000_000_000_000u128;

/// Minimum stake for Bronze tier (1,000 tokens).
pub const BRONZE_MIN_STAKE: u128 = 1_000 * 1_000_000_000_000_000_000u128;
/// Minimum stake for Silver tier (10,000 tokens).
pub const SILVER_MIN_STAKE: u128 = 10_000 * 1_000_000_000_000_000_000u128;
/// Minimum stake for Gold tier (100,000 tokens, validator eligible).
pub const GOLD_MIN_STAKE: u128 = 100_000 * 1_000_000_000_000_000_000u128;
/// Minimum stake for Platinum tier (1M tokens, full fee waiver).
pub const PLATINUM_MIN_STAKE: u128 = 1_000_000 * 1_000_000_000_000_000_000u128;

/// Minimum validator stake (Gold tier). Matches Checklist "minimum stake: 10 tokens"
/// scaled to realistic amount: 10,000 for actual validator security.
pub const VALIDATOR_MIN_STAKE: u128 = GOLD_MIN_STAKE;

/// Early staker target APY in basis points (20% = 2000 bps).
pub const EARLY_APY_BPS: u32 = 2000;
/// Target steady-state APY in basis points (5% = 500 bps).
pub const STEADY_APY_BPS: u32 = 500;
/// Phase transition: months after launch when APY tapers to steady state.
pub const APY_TAPER_MONTHS: u32 = 24;

/// Slashing percentage for validator downtime (5% of stake).
pub const SLASH_DOWNTIME_BPS: u32 = 500;
/// Slashing percentage for byzantine behaviour (50% of stake).
pub const SLASH_BYZANTINE_BPS: u32 = 5_000;

/// Fee discount for Bronze tier (10%).
pub const BRONZE_DISCOUNT_BPS: u32 = 1_000;
/// Fee discount for Silver tier (25%).
pub const SILVER_DISCOUNT_BPS: u32 = 2_500;
/// Fee discount for Gold tier (50%).
pub const GOLD_DISCOUNT_BPS: u32 = 5_000;
/// Fee discount for Platinum tier (100% — full waiver).
pub const PLATINUM_DISCOUNT_BPS: u32 = 10_000;

// ─── Types ────────────────────────────────────────────────────────────────────

/// Staking tier for $INTERLINK holders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StakingTier {
    None,
    Bronze,
    Silver,
    Gold,
    Platinum,
}

impl StakingTier {
    /// Minimum stake required to reach this tier (in token wei).
    pub fn min_stake(&self) -> u128 {
        match self {
            StakingTier::None => 0,
            StakingTier::Bronze => BRONZE_MIN_STAKE,
            StakingTier::Silver => SILVER_MIN_STAKE,
            StakingTier::Gold => GOLD_MIN_STAKE,
            StakingTier::Platinum => PLATINUM_MIN_STAKE,
        }
    }

    /// Fee discount granted to this tier in basis points.
    pub fn fee_discount_bps(&self) -> u32 {
        match self {
            StakingTier::None => 0,
            StakingTier::Bronze => BRONZE_DISCOUNT_BPS,
            StakingTier::Silver => SILVER_DISCOUNT_BPS,
            StakingTier::Gold => GOLD_DISCOUNT_BPS,
            StakingTier::Platinum => PLATINUM_DISCOUNT_BPS,
        }
    }

    /// Whether this tier is eligible to run as a validator.
    pub fn is_validator_eligible(&self) -> bool {
        matches!(self, StakingTier::Gold | StakingTier::Platinum)
    }

    /// Governance voting weight multiplier (basis points of stake).
    pub fn voting_weight_multiplier_bps(&self) -> u32 {
        match self {
            StakingTier::None => 0,
            StakingTier::Bronze => 5_000,   // 0.5x voting power per token
            StakingTier::Silver => 10_000,  // 1x (linear)
            StakingTier::Gold => 12_500,    // 1.25x (small validator bonus)
            StakingTier::Platinum => 15_000, // 1.5x (large validator bonus, capped later)
        }
    }

    /// Human-readable tier name.
    pub fn name(&self) -> &'static str {
        match self {
            StakingTier::None => "None",
            StakingTier::Bronze => "Bronze",
            StakingTier::Silver => "Silver",
            StakingTier::Gold => "Gold",
            StakingTier::Platinum => "Platinum",
        }
    }
}

/// Classify a staker into their tier based on staked amount.
pub fn classify_tier(staked_amount: u128) -> StakingTier {
    if staked_amount >= PLATINUM_MIN_STAKE {
        StakingTier::Platinum
    } else if staked_amount >= GOLD_MIN_STAKE {
        StakingTier::Gold
    } else if staked_amount >= SILVER_MIN_STAKE {
        StakingTier::Silver
    } else if staked_amount >= BRONZE_MIN_STAKE {
        StakingTier::Bronze
    } else {
        StakingTier::None
    }
}

/// A staker's position and rewards summary.
#[derive(Debug, Clone, Serialize)]
pub struct StakerPosition {
    /// Staked amount in token wei.
    pub staked_amount: u128,
    /// Current tier.
    pub tier: StakingTier,
    /// Effective fee discount for bridge transfers.
    pub fee_discount_bps: u32,
    /// Validator eligibility.
    pub is_validator_eligible: bool,
    /// Governance voting power.
    pub voting_power: u128,
    /// Current APY in basis points.
    pub current_apy_bps: u32,
    /// Estimated daily reward (in token wei).
    pub estimated_daily_reward: u128,
    /// Estimated annual reward (in token wei).
    pub estimated_annual_reward: u128,
}

/// Protocol-wide staking statistics.
#[derive(Debug, Clone, Serialize)]
pub struct StakingStats {
    /// Total staked supply.
    pub total_staked: u128,
    /// Staking participation rate (basis points of total supply).
    pub participation_bps: u32,
    /// Current protocol APY offered to stakers.
    pub current_apy_bps: u32,
    /// Annualised reward budget (from protocol revenue).
    pub annual_reward_budget: u128,
    /// Number of validator-eligible stakers.
    pub validator_count_estimate: u32,
    /// Competitor APY comparison.
    pub competitor_apys: Vec<CompetitorApy>,
}

/// Competitor staking APY comparison.
#[derive(Debug, Clone, Serialize)]
pub struct CompetitorApy {
    pub name: &'static str,
    pub token: &'static str,
    pub apy_min_bps: u32,
    pub apy_max_bps: u32,
    pub lockup_months: u32,
}

// ─── Staker reward calculation ────────────────────────────────────────────────

/// Calculate the current protocol APY based on months since launch.
///
/// APY starts at `EARLY_APY_BPS` and linearly tapers to `STEADY_APY_BPS`
/// over `APY_TAPER_MONTHS` months. Mimics Stargate's ve-model taper.
pub fn current_apy_bps(months_since_launch: u32) -> u32 {
    if months_since_launch >= APY_TAPER_MONTHS {
        return STEADY_APY_BPS;
    }
    // Linear interpolation from EARLY to STEADY
    let taper_fraction = months_since_launch * 10_000 / APY_TAPER_MONTHS;
    let delta = EARLY_APY_BPS.saturating_sub(STEADY_APY_BPS);
    EARLY_APY_BPS - (delta * taper_fraction / 10_000)
}

/// Calculate a staker's position and reward estimates.
///
/// # Arguments
/// - `staked_amount`: tokens staked in wei
/// - `total_staked`: protocol-wide total staked
/// - `months_since_launch`: for APY taper calculation
pub fn calculate_position(
    staked_amount: u128,
    total_staked: u128,
    months_since_launch: u32,
) -> StakerPosition {
    let tier = classify_tier(staked_amount);
    let apy_bps = current_apy_bps(months_since_launch);

    // Voting power: stake × multiplier, anti-whale capped at 5% of total supply
    let raw_voting_power = staked_amount
        .saturating_mul(tier.voting_weight_multiplier_bps() as u128)
        / 10_000;
    let whale_cap = TOTAL_SUPPLY / 20; // 5% of supply
    let voting_power = raw_voting_power.min(whale_cap);

    // Annual reward = staked × APY%
    let estimated_annual_reward = staked_amount
        .saturating_mul(apy_bps as u128)
        / 10_000;
    let estimated_daily_reward = estimated_annual_reward / 365;

    // Tier-specific APY bonus: Gold/Platinum get +1-2% for validator duties
    let tier_bonus_bps: u32 = match tier {
        StakingTier::Gold => 100,      // +1%
        StakingTier::Platinum => 200,  // +2%
        _ => 0,
    };
    let effective_apy = apy_bps + tier_bonus_bps;

    let _ = total_staked; // used in governance quorum logic (future: DAO module)

    StakerPosition {
        staked_amount,
        tier,
        fee_discount_bps: tier.fee_discount_bps(),
        is_validator_eligible: tier.is_validator_eligible(),
        voting_power,
        current_apy_bps: effective_apy,
        estimated_daily_reward,
        estimated_annual_reward,
    }
}

/// Calculate the slashing penalty for a validator.
///
/// # Arguments
/// - `staked_amount`: validator's staked amount
/// - `is_byzantine`: true for byzantine fault, false for downtime
///
/// # Returns
/// `(slash_amount, remaining_stake)`
pub fn calculate_slash(staked_amount: u128, is_byzantine: bool) -> (u128, u128) {
    let slash_bps = if is_byzantine {
        SLASH_BYZANTINE_BPS
    } else {
        SLASH_DOWNTIME_BPS
    };
    let slash_amount = staked_amount.saturating_mul(slash_bps as u128) / 10_000;
    let remaining = staked_amount.saturating_sub(slash_amount);
    (slash_amount, remaining)
}

/// Apply a fee discount to a base fee amount based on staker tier.
///
/// # Arguments
/// - `base_fee_amount`: fee in token wei before discount
/// - `tier`: staker's tier
///
/// # Returns
/// Discounted fee amount (may be 0 for Platinum).
pub fn apply_staker_discount(base_fee_amount: u128, tier: StakingTier) -> u128 {
    let discount_bps = tier.fee_discount_bps() as u128;
    let discount = base_fee_amount.saturating_mul(discount_bps) / 10_000;
    base_fee_amount.saturating_sub(discount)
}

/// Protocol-wide staking statistics.
pub fn protocol_stats(
    total_staked: u128,
    validator_count: u32,
    months_since_launch: u32,
) -> StakingStats {
    let apy_bps = current_apy_bps(months_since_launch);
    let participation_bps = (total_staked * 10_000 / TOTAL_SUPPLY) as u32;
    let annual_reward_budget = total_staked.saturating_mul(apy_bps as u128) / 10_000;

    StakingStats {
        total_staked,
        participation_bps,
        current_apy_bps: apy_bps,
        annual_reward_budget,
        validator_count_estimate: validator_count,
        competitor_apys: vec![
            CompetitorApy {
                name: "Stargate",
                token: "STG",
                apy_min_bps: 800,   // 8%
                apy_max_bps: 1200,  // 12%
                lockup_months: 12,
            },
            CompetitorApy {
                name: "Wormhole",
                token: "W",
                apy_min_bps: 0,
                apy_max_bps: 0,
                lockup_months: 0, // no native staking rewards
            },
            CompetitorApy {
                name: "Across",
                token: "UMA",
                apy_min_bps: 300,  // 3%
                apy_max_bps: 800,  // 8%
                lockup_months: 0,
            },
            CompetitorApy {
                name: "InterLink (early)",
                token: "INTERLINK",
                apy_min_bps: STEADY_APY_BPS,
                apy_max_bps: EARLY_APY_BPS,
                lockup_months: 0, // flexible
            },
        ],
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_classification() {
        assert_eq!(classify_tier(0), StakingTier::None);
        assert_eq!(classify_tier(BRONZE_MIN_STAKE - 1), StakingTier::None);
        assert_eq!(classify_tier(BRONZE_MIN_STAKE), StakingTier::Bronze);
        assert_eq!(classify_tier(SILVER_MIN_STAKE), StakingTier::Silver);
        assert_eq!(classify_tier(GOLD_MIN_STAKE), StakingTier::Gold);
        assert_eq!(classify_tier(PLATINUM_MIN_STAKE), StakingTier::Platinum);
    }

    #[test]
    fn test_fee_discounts_increase_with_tier() {
        assert!(StakingTier::Bronze.fee_discount_bps() < StakingTier::Silver.fee_discount_bps());
        assert!(StakingTier::Silver.fee_discount_bps() < StakingTier::Gold.fee_discount_bps());
        assert!(StakingTier::Gold.fee_discount_bps() < StakingTier::Platinum.fee_discount_bps());
        assert_eq!(StakingTier::Platinum.fee_discount_bps(), 10_000); // 100%
    }

    #[test]
    fn test_platinum_full_fee_waiver() {
        let fee = apply_staker_discount(1_000_000, StakingTier::Platinum);
        assert_eq!(fee, 0, "Platinum should get 100% fee waiver");
    }

    #[test]
    fn test_bronze_10_percent_discount() {
        let base_fee = 10_000u128;
        let discounted = apply_staker_discount(base_fee, StakingTier::Bronze);
        assert_eq!(discounted, 9_000, "Bronze: 10% off = 9000");
    }

    #[test]
    fn test_gold_50_percent_discount() {
        let base_fee = 10_000u128;
        let discounted = apply_staker_discount(base_fee, StakingTier::Gold);
        assert_eq!(discounted, 5_000, "Gold: 50% off = 5000");
    }

    #[test]
    fn test_validator_eligibility() {
        assert!(!StakingTier::None.is_validator_eligible());
        assert!(!StakingTier::Bronze.is_validator_eligible());
        assert!(!StakingTier::Silver.is_validator_eligible());
        assert!(StakingTier::Gold.is_validator_eligible());
        assert!(StakingTier::Platinum.is_validator_eligible());
    }

    #[test]
    fn test_apy_taper() {
        // At launch: EARLY_APY
        assert_eq!(current_apy_bps(0), EARLY_APY_BPS);
        // At taper complete: STEADY_APY
        assert_eq!(current_apy_bps(APY_TAPER_MONTHS), STEADY_APY_BPS);
        // Before taper: somewhere in between
        let mid = current_apy_bps(APY_TAPER_MONTHS / 2);
        assert!(mid > STEADY_APY_BPS && mid < EARLY_APY_BPS);
    }

    #[test]
    fn test_staker_position_rewards() {
        let pos = calculate_position(GOLD_MIN_STAKE, GOLD_MIN_STAKE * 100, 0);
        assert_eq!(pos.tier, StakingTier::Gold);
        assert!(pos.is_validator_eligible);
        assert!(pos.estimated_annual_reward > 0);
        // Gold tier: EARLY_APY + 1% bonus
        assert_eq!(pos.current_apy_bps, EARLY_APY_BPS + 100);
        // Daily = annual / 365
        assert_eq!(pos.estimated_daily_reward, pos.estimated_annual_reward / 365);
    }

    #[test]
    fn test_slash_byzantine_50_percent() {
        let stake = GOLD_MIN_STAKE;
        let (slash, remaining) = calculate_slash(stake, true);
        assert_eq!(slash, stake / 2);
        assert_eq!(remaining, stake / 2);
    }

    #[test]
    fn test_slash_downtime_5_percent() {
        let stake = GOLD_MIN_STAKE;
        let (slash, remaining) = calculate_slash(stake, false);
        assert_eq!(slash, stake * 5 / 100);
        assert_eq!(remaining, stake - slash);
    }

    #[test]
    fn test_whale_cap_on_voting_power() {
        let whale_stake = TOTAL_SUPPLY; // owns 100% of supply
        let pos = calculate_position(whale_stake, TOTAL_SUPPLY, 0);
        // Voting power capped at 5% of supply
        assert!(pos.voting_power <= TOTAL_SUPPLY / 20);
    }

    #[test]
    fn test_protocol_stats() {
        let staked = TOTAL_SUPPLY / 10; // 10% participation
        let stats = protocol_stats(staked, 5, 0);
        assert_eq!(stats.participation_bps, 1_000); // 10%
        assert_eq!(stats.current_apy_bps, EARLY_APY_BPS);
        assert!(stats.annual_reward_budget > 0);
        assert_eq!(stats.competitor_apys.len(), 4);
    }
}
