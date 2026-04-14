//! MEV capture & LP fee breakeven analysis for the InterLink bridge.
//!
//! Answers the checklist question: at what bridge volume do MEV capture +
//! LP yield fees cover the cost of running the relayer, so small transfers
//! can remain free (Tier 1: 0%)?
//!
//! # Economic model
//!
//! Revenue sources for InterLink validators / LP providers:
//!   1. Protocol fees (0.05% Tier 2, 0.01% Tier 3)
//!   2. MEV capture: front-run / back-run arbitrage on the destination chain
//!      after a cross-chain swap event is seen but before it settles
//!   3. LP float yield: idle collateral earns yield while locked in the vault
//!
//! # Breakeven definition
//!
//! The relayer costs `C` per day (proof compute + RPC calls + Solana gas).
//! We need total revenue R ≥ C to sustain the 0% Tier-1 model.
//! Breakeven volume = C / (weighted_average_fee_rate + mev_rate + lp_yield_rate)
//!
//! # Competitive comparison
//!
//! | Bridge    | MEV capture | LP yield | Fee income  |
//! |-----------|-------------|----------|-------------|
//! | Wormhole  | None        | None     | $1-20/VAA   |
//! | Across    | None        | 3-8% APY | 0.25-1%     |
//! | Stargate  | None        | 4-10%    | 0.5-5%      |
//! | InterLink | Active      | 3-5%     | 0-0.05%     |

use crate::fee::FeeTier;
use serde::Serialize;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Daily relayer operating cost in USD cents.
/// Breakdown:
///   - Proof compute (cloud GPU/CPU): ~$3/day
///   - RPC subscriptions (Alchemy/QuickNode): ~$2/day
///   - Solana transaction fees: ~$0.50/day (200k CU × $0.000025 avg)
///   - Total: ~$5.50/day ≈ 550 cents
pub const DAILY_OPERATING_COST_CENTS: u64 = 550;

/// MEV capture rate: fraction of swap value captured via arbitrage.
/// Estimated 0.01–0.05% per swap on liquid DEX pairs.
/// Conservative: 0.01% (1 basis point).
pub const MEV_CAPTURE_BPS: u32 = 1;

/// LP float yield rate (annualised basis points).
/// Collateral locked in bridge vault earns ~3% APY via integration with Aave/Compound.
/// 3% APY → 0.0082% per day → ~0.82 basis points per day
pub const LP_ANNUAL_YIELD_BPS: u32 = 300; // 3% APY

/// Competitor LP APY benchmarks (basis points per annum).
pub const ACROSS_LP_APY_MIN_BPS: u32 = 300; // 3%
pub const ACROSS_LP_APY_MAX_BPS: u32 = 800; // 8%
pub const STARGATE_LP_APY_MIN_BPS: u32 = 400; // 4%
pub const STARGATE_LP_APY_MAX_BPS: u32 = 1000; // 10%

/// Number of trading days per year.
pub const TRADING_DAYS_PER_YEAR: u64 = 365;

// ─── Types ────────────────────────────────────────────────────────────────────

/// Full MEV + LP + fee revenue model for a given daily volume.
#[derive(Debug, Clone, Serialize)]
pub struct RevenueBreakdown {
    /// Daily bridge volume in USD cents.
    pub daily_volume_cents: u64,
    /// Revenue from protocol fees (weighted across tiers).
    pub fee_revenue_cents: u64,
    /// Revenue from MEV capture.
    pub mev_revenue_cents: u64,
    /// Revenue from LP yield on locked collateral.
    pub lp_yield_revenue_cents: u64,
    /// Total daily revenue.
    pub total_revenue_cents: u64,
    /// Daily operating cost.
    pub operating_cost_cents: u64,
    /// Net profit/loss (positive = profitable).
    pub net_cents: i64,
    /// Break-even status.
    pub is_breakeven: bool,
    /// Profit margin as basis points of volume.
    pub margin_bps: i32,
}

/// Breakeven analysis: minimum volume needed to cover costs.
#[derive(Debug, Clone, Serialize)]
pub struct BreakevenAnalysis {
    /// Minimum daily volume (USD cents) to cover operating costs.
    pub breakeven_volume_cents: u64,
    /// Volume at which Tier 1 transfers are subsidised by larger transfers.
    pub tier1_subsidy_breakeven_cents: u64,
    /// Annualised revenue at breakeven volume.
    pub annual_revenue_at_breakeven_cents: u64,
    /// Revenue sources breakdown at breakeven.
    pub revenue_at_breakeven: RevenueBreakdown,
    /// Competitor comparison.
    pub competitor_breakevens: Vec<CompetitorBreakeven>,
}

/// Breakeven comparison with a competitor bridge.
#[derive(Debug, Clone, Serialize)]
pub struct CompetitorBreakeven {
    pub name: &'static str,
    /// Their fee rate (bps).
    pub fee_bps: u32,
    /// Their LP APY (bps).
    pub lp_apy_bps: u32,
    /// Their estimated daily operating cost (cents).
    pub daily_cost_cents: u64,
    /// Their breakeven daily volume (cents).
    pub breakeven_volume_cents: u64,
}

// ─── Revenue estimation ───────────────────────────────────────────────────────

/// Estimate daily revenue breakdown for a given bridge volume.
///
/// # Arguments
/// - `daily_volume_cents`: total USD value bridged today (in cents)
/// - `avg_locked_collateral_cents`: average collateral locked at any time (for LP yield)
pub fn estimate_daily_revenue(
    daily_volume_cents: u64,
    avg_locked_collateral_cents: u64,
) -> RevenueBreakdown {
    // Fee revenue: apply weighted average fee rate across tiers.
    // Assume distribution: 40% Tier 1 ($0 fee), 50% Tier 2 (0.05%), 10% Tier 3 (0.01%)
    let tier2_volume = daily_volume_cents * 50 / 100;
    let tier3_volume = daily_volume_cents * 10 / 100;
    let fee_revenue_cents = (tier2_volume as u128 * FeeTier::Standard.bps() as u128 / 10_000
        + tier3_volume as u128 * FeeTier::Institutional.bps() as u128 / 10_000)
        as u64;

    // MEV capture: applied to swap volume (assume 60% of transfers are swaps).
    let swap_volume = daily_volume_cents * 60 / 100;
    let mev_revenue_cents = (swap_volume as u128 * MEV_CAPTURE_BPS as u128 / 10_000) as u64;

    // LP yield: daily portion of annual yield on locked collateral.
    let lp_yield_revenue_cents = (avg_locked_collateral_cents as u128 * LP_ANNUAL_YIELD_BPS as u128
        / 10_000
        / TRADING_DAYS_PER_YEAR as u128) as u64;

    let total_revenue_cents = fee_revenue_cents + mev_revenue_cents + lp_yield_revenue_cents;
    let net_cents = total_revenue_cents as i64 - DAILY_OPERATING_COST_CENTS as i64;

    // Margin in basis points of daily volume
    let margin_bps = if daily_volume_cents > 0 {
        (net_cents * 10_000 / daily_volume_cents as i64) as i32
    } else {
        -10_000 // -100% if no volume
    };

    RevenueBreakdown {
        daily_volume_cents,
        fee_revenue_cents,
        mev_revenue_cents,
        lp_yield_revenue_cents,
        total_revenue_cents,
        operating_cost_cents: DAILY_OPERATING_COST_CENTS,
        net_cents,
        is_breakeven: net_cents >= 0,
        margin_bps,
    }
}

// ─── Breakeven solver ─────────────────────────────────────────────────────────

/// Find the minimum daily bridge volume needed to cover operating costs.
///
/// Uses binary search over volume space: $0 – $100M/day.
pub fn calculate_breakeven() -> BreakevenAnalysis {
    // Binary search for breakeven volume.
    // Assume avg locked collateral ≈ 10% of daily volume (funds locked ~2.4h avg).
    let collateral_fraction = 10u64; // 10%

    let mut lo: u64 = 0;
    let mut hi: u64 = 10_000_000_000u64; // $100M in cents

    while hi - lo > 100 {
        let mid = (lo + hi) / 2;
        let avg_collateral = mid * collateral_fraction / 100;
        let rev = estimate_daily_revenue(mid, avg_collateral);
        if rev.is_breakeven {
            hi = mid;
        } else {
            lo = mid;
        }
    }

    let breakeven_volume_cents = hi;
    let avg_collateral_at_breakeven = breakeven_volume_cents * collateral_fraction / 100;
    let revenue_at_breakeven =
        estimate_daily_revenue(breakeven_volume_cents, avg_collateral_at_breakeven);

    // Tier 1 subsidy breakeven: volume at which Standard/Institutional fees cover
    // both operating costs AND the "lost" revenue from Tier 1 free transfers.
    // Tier 1 is ~40% of volume. If those transferred normally at 0.05%, cost would be:
    //   tier1_volume * 5bps / 10_000 = forgone_revenue
    // Breakeven: fee_revenue > operating_cost + forgone_revenue
    let tier1_subsidy_breakeven_cents = breakeven_volume_cents * 150 / 100; // ~50% higher

    // Competitor breakevens (simplified: only fee + LP yield, no MEV)
    let competitors = vec![
        CompetitorBreakeven {
            name: "Wormhole",
            fee_bps: 10,
            lp_apy_bps: 0,
            daily_cost_cents: 5_000, // ~$50/day (19 guardians + infra)
            breakeven_volume_cents: {
                // Wormhole charges flat $1/VAA, not volume-based
                // At 19 validators: breakeven ~$50/day ÷ $1/VAA = 50 VAAs/day
                50 * 100 // 50 VAAs × avg $1 = $50 worth of fees
            },
        },
        CompetitorBreakeven {
            name: "Across",
            fee_bps: ACROSS_LP_APY_MIN_BPS / TRADING_DAYS_PER_YEAR as u32,
            lp_apy_bps: ACROSS_LP_APY_MIN_BPS,
            daily_cost_cents: 2_000, // ~$20/day (optimistic rollup infra)
            breakeven_volume_cents: {
                // Across: 0.25% fee. Break-even at $20/day cost:
                // volume × 0.0025 = $0.20 → volume = $80/day
                2_000 * 10_000 / 25 // cost × 10000 / fee_bps
            },
        },
        CompetitorBreakeven {
            name: "Stargate v2",
            fee_bps: STARGATE_LP_APY_MIN_BPS / TRADING_DAYS_PER_YEAR as u32,
            lp_apy_bps: STARGATE_LP_APY_MIN_BPS,
            daily_cost_cents: 3_000, // ~$30/day
            breakeven_volume_cents: {
                // Stargate: 0.5% fee. Break-even at $30/day cost:
                // volume × 0.005 = $0.30 → volume = $60/day
                3_000 * 10_000 / 50
            },
        },
    ];

    BreakevenAnalysis {
        breakeven_volume_cents,
        tier1_subsidy_breakeven_cents,
        annual_revenue_at_breakeven_cents: revenue_at_breakeven.total_revenue_cents
            * TRADING_DAYS_PER_YEAR,
        revenue_at_breakeven,
        competitor_breakevens: competitors,
    }
}

/// Format a human-readable breakeven summary.
pub fn format_breakeven_report() -> String {
    let analysis = calculate_breakeven();
    let mut out = String::new();

    out.push_str("\n┌─ InterLink MEV + LP Fee Breakeven Analysis ─────────────────────────┐\n");
    out.push_str(&format!(
        "│ Daily operating cost:          ${:.2}                                │\n",
        DAILY_OPERATING_COST_CENTS as f64 / 100.0
    ));
    out.push_str(&format!(
        "│ Breakeven daily volume:        ${:.2}                              │\n",
        analysis.breakeven_volume_cents as f64 / 100.0
    ));
    out.push_str(&format!(
        "│ Tier-1 subsidy breakeven:      ${:.2}                              │\n",
        analysis.tier1_subsidy_breakeven_cents as f64 / 100.0
    ));
    out.push_str("│                                                                      │\n");

    let r = &analysis.revenue_at_breakeven;
    out.push_str("│ Revenue breakdown at breakeven:                                      │\n");
    out.push_str(&format!(
        "│   Protocol fees:    ${:.4}                                       │\n",
        r.fee_revenue_cents as f64 / 100.0
    ));
    out.push_str(&format!(
        "│   MEV capture:      ${:.4}                                       │\n",
        r.mev_revenue_cents as f64 / 100.0
    ));
    out.push_str(&format!(
        "│   LP yield:         ${:.4}                                       │\n",
        r.lp_yield_revenue_cents as f64 / 100.0
    ));
    out.push_str("│                                                                      │\n");
    out.push_str("│ Competitor breakeven volumes:                                        │\n");
    for c in &analysis.competitor_breakevens {
        out.push_str(&format!(
            "│   {:12} ${:.2} (fee: {} bps, APY: {} bps)         │\n",
            c.name,
            c.breakeven_volume_cents as f64 / 100.0,
            c.fee_bps,
            c.lp_apy_bps,
        ));
    }
    out.push_str("└──────────────────────────────────────────────────────────────────────┘\n");
    out
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_breakeven_is_positive() {
        let analysis = calculate_breakeven();
        // Breakeven volume must be positive and finite
        assert!(analysis.breakeven_volume_cents > 0);
        // Revenue at breakeven must cover costs
        assert!(analysis.revenue_at_breakeven.is_breakeven);
        // Tier-1 subsidy breakeven is higher than base breakeven
        assert!(analysis.tier1_subsidy_breakeven_cents > analysis.breakeven_volume_cents);
    }

    #[test]
    fn test_zero_volume_is_loss() {
        let rev = estimate_daily_revenue(0, 0);
        assert!(!rev.is_breakeven);
        assert!(rev.net_cents < 0);
    }

    #[test]
    fn test_high_volume_is_profitable() {
        // $10M/day should easily cover $5.50/day costs
        let rev = estimate_daily_revenue(1_000_000_000, 100_000_000); // $10M
        assert!(rev.is_breakeven);
        assert!(rev.net_cents > 0);
        assert!(rev.total_revenue_cents > DAILY_OPERATING_COST_CENTS);
    }

    #[test]
    fn test_mev_revenue_positive_for_swaps() {
        // Any non-zero volume should generate MEV capture
        let rev = estimate_daily_revenue(10_000_000, 1_000_000); // $100k/day
        assert!(
            rev.mev_revenue_cents > 0,
            "MEV should be positive for non-zero volume"
        );
    }

    #[test]
    fn test_lp_yield_grows_with_collateral() {
        let rev_low = estimate_daily_revenue(100_000_000, 1_000_000); // $10k collateral
        let rev_high = estimate_daily_revenue(100_000_000, 100_000_000); // $1M collateral
        assert!(
            rev_high.lp_yield_revenue_cents > rev_low.lp_yield_revenue_cents,
            "Higher collateral should yield more LP revenue"
        );
    }

    #[test]
    fn test_competitor_breakevens_populated() {
        let analysis = calculate_breakeven();
        assert_eq!(analysis.competitor_breakevens.len(), 3);
        let names: Vec<&str> = analysis
            .competitor_breakevens
            .iter()
            .map(|c| c.name)
            .collect();
        assert!(names.contains(&"Wormhole"));
        assert!(names.contains(&"Across"));
        assert!(names.contains(&"Stargate v2"));
    }

    #[test]
    fn test_breakeven_report_format() {
        let report = format_breakeven_report();
        assert!(report.contains("InterLink MEV"));
        assert!(report.contains("Breakeven daily volume"));
        assert!(report.contains("MEV capture"));
        assert!(report.contains("LP yield"));
    }

    #[test]
    fn test_fee_revenue_tier_weighting() {
        // At $1M volume: 50% Tier 2 at 5 bps = $2.50, 10% Tier 3 at 1 bps = $0.10
        let rev = estimate_daily_revenue(100_000_000, 0); // $1M volume
        assert!(rev.fee_revenue_cents > 0);
        // Tier 1 (40%) contributes $0 fee revenue
        // Tier 2 (50%) = 50_000_000 * 5 / 10000 = 25000 cents = $250
        // Tier 3 (10%) = 10_000_000 * 1 / 10000 = 1000 cents = $10
        assert_eq!(rev.fee_revenue_cents, 25_000 + 1_000);
    }
}
