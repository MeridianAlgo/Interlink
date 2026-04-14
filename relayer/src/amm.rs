//! Liquidity AMM for InterLink bridge capital efficiency (Phase 4).
//!
//! Implements a constant-product AMM (Uniswap v2 style) for the bridge vault,
//! so idle liquidity earns yield while transfers are in-flight.
//!
//! # Why an AMM?
//!
//! Across Protocol uses LP pools to earn yield on locked capital (3-8% APY).
//! Stargate uses unified liquidity across chains (4-10% APY).
//! InterLink's AMM lets bridge LPs earn fees on internal swaps + cross-chain
//! routing decisions — without the fragmented liquidity problem.
//!
//! # Model
//!
//! Each token pair has a pool: (reserve_a, reserve_b).
//! The invariant is: reserve_a × reserve_b = k (constant product).
//!
//! Swaps: given Δa in, Δb out = reserve_b × Δa / (reserve_a + Δa) × (1 - fee)
//!
//! LP fee = 0.30% (30 bps) — split:
//!   0.25% to LPs (incentivizes liquidity)
//!   0.05% to protocol treasury (InterLink fee)
//!
//! # Competitive comparison
//! | Bridge    | LP APY  | Liquidity fragmentation |
//! |-----------|---------|-------------------------|
//! | Across    | 3-8%    | Per-chain pool           |
//! | Stargate  | 4-10%   | Unified (Delta)          |
//! | Uniswap   | 0.3%    | Per-pair pool            |
//! | InterLink | 3-5%    | Cross-chain unified pool |

use serde::Serialize;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Total LP fee in basis points (0.30%).
pub const LP_FEE_BPS: u32 = 30;
/// Fraction of LP fee going to LPs in basis points (0.25%).
pub const LP_SHARE_BPS: u32 = 25;
/// Fraction of LP fee going to protocol treasury (0.05%).
pub const PROTOCOL_SHARE_BPS: u32 = 5;

/// Minimum liquidity locked forever to prevent division-by-zero (MINIMUM_LIQUIDITY).
pub const MINIMUM_LIQUIDITY: u128 = 1_000;

/// Maximum price impact allowed per swap (5%). Reject swaps with higher impact.
pub const MAX_PRICE_IMPACT_BPS: u32 = 500;

// ─── Types ────────────────────────────────────────────────────────────────────

/// A unique identifier for a token pair pool.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct PoolId {
    /// Address of token A (lower address first for canonical ordering).
    pub token_a: [u8; 20],
    /// Address of token B.
    pub token_b: [u8; 20],
    /// Source chain for token A.
    pub chain_a: u64,
    /// Source chain for token B.
    pub chain_b: u64,
}

impl PoolId {
    /// Create a pool ID, canonically ordering tokens (lower address first).
    pub fn new(mut token_a: [u8; 20], chain_a: u64, mut token_b: [u8; 20], chain_b: u64) -> Self {
        // Canonical ordering: token_a < token_b lexicographically
        if (token_a, chain_a) > (token_b, chain_b) {
            std::mem::swap(&mut token_a, &mut token_b);
        }
        Self {
            token_a,
            chain_a,
            token_b,
            chain_b,
        }
    }
}

/// AMM errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmmError {
    InsufficientLiquidity,
    ZeroAmount,
    SlippageExceeded { actual_bps: u32, max_bps: u32 },
    PriceImpactTooHigh { impact_bps: u32 },
    InsufficientLpShares { have: u128, need: u128 },
    InvalidToken,
    Overflow,
}

impl std::fmt::Display for AmmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AmmError::InsufficientLiquidity => write!(f, "pool has insufficient liquidity"),
            AmmError::ZeroAmount => write!(f, "amount must be > 0"),
            AmmError::SlippageExceeded {
                actual_bps,
                max_bps,
            } => {
                write!(f, "slippage {actual_bps} bps exceeds max {max_bps} bps")
            }
            AmmError::PriceImpactTooHigh { impact_bps } => {
                write!(f, "price impact {impact_bps} bps exceeds 500 bps limit")
            }
            AmmError::InsufficientLpShares { have, need } => {
                write!(f, "insufficient LP shares: have {have}, need {need}")
            }
            AmmError::InvalidToken => write!(f, "token not in this pool"),
            AmmError::Overflow => write!(f, "arithmetic overflow"),
        }
    }
}

/// A single AMM liquidity pool (constant product x*y=k).
#[derive(Debug, Clone, Serialize)]
pub struct Pool {
    pub id: PoolId,
    /// Reserve of token A.
    pub reserve_a: u128,
    /// Reserve of token B.
    pub reserve_b: u128,
    /// Total LP shares outstanding.
    pub total_shares: u128,
    /// Total protocol fees collected (token A denomination).
    pub fees_collected_a: u128,
    /// Total protocol fees collected (token B denomination).
    pub fees_collected_b: u128,
    /// Total volume through this pool (token A denomination).
    pub volume_a: u128,
}

impl Pool {
    /// Create a new empty pool.
    pub fn new(id: PoolId) -> Self {
        Self {
            id,
            reserve_a: 0,
            reserve_b: 0,
            total_shares: 0,
            fees_collected_a: 0,
            fees_collected_b: 0,
            volume_a: 0,
        }
    }

    /// Invariant k = reserve_a × reserve_b.
    pub fn k(&self) -> u128 {
        self.reserve_a.saturating_mul(self.reserve_b)
    }

    /// Current spot price: how many token B per 1 token A (scaled × 1e6).
    pub fn spot_price_a_in_b_e6(&self) -> u128 {
        if self.reserve_a == 0 {
            return 0;
        }
        self.reserve_b.saturating_mul(1_000_000) / self.reserve_a
    }

    /// Add initial liquidity (first deposit sets the price).
    ///
    /// Returns LP shares minted.
    pub fn add_initial_liquidity(
        &mut self,
        amount_a: u128,
        amount_b: u128,
    ) -> Result<u128, AmmError> {
        if amount_a == 0 || amount_b == 0 {
            return Err(AmmError::ZeroAmount);
        }
        debug_assert_eq!(self.total_shares, 0, "use add_liquidity for existing pools");

        // Shares = sqrt(amount_a × amount_b) - MINIMUM_LIQUIDITY (locked forever)
        let shares_raw = isqrt(amount_a.saturating_mul(amount_b));
        let shares = shares_raw.saturating_sub(MINIMUM_LIQUIDITY);
        if shares == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }

        self.reserve_a = amount_a;
        self.reserve_b = amount_b;
        self.total_shares = shares + MINIMUM_LIQUIDITY; // MINIMUM_LIQUIDITY locked
        Ok(shares)
    }

    /// Add liquidity to an existing pool (proportional deposit).
    ///
    /// Returns (actual_a_deposited, actual_b_deposited, shares_minted).
    pub fn add_liquidity(
        &mut self,
        amount_a: u128,
        amount_b: u128,
    ) -> Result<(u128, u128, u128), AmmError> {
        if amount_a == 0 || amount_b == 0 {
            return Err(AmmError::ZeroAmount);
        }
        if self.reserve_a == 0 {
            let shares = self.add_initial_liquidity(amount_a, amount_b)?;
            return Ok((amount_a, amount_b, shares));
        }

        // Optimal amount_b given amount_a (maintain ratio)
        let optimal_b = amount_a
            .saturating_mul(self.reserve_b)
            .checked_div(self.reserve_a)
            .ok_or(AmmError::Overflow)?;

        let (actual_a, actual_b) = if optimal_b <= amount_b {
            (amount_a, optimal_b)
        } else {
            let optimal_a = amount_b
                .saturating_mul(self.reserve_a)
                .checked_div(self.reserve_b)
                .ok_or(AmmError::Overflow)?;
            (optimal_a, amount_b)
        };

        // Shares proportional to deposit fraction
        let shares = actual_a
            .saturating_mul(self.total_shares)
            .checked_div(self.reserve_a)
            .ok_or(AmmError::Overflow)?;

        self.reserve_a += actual_a;
        self.reserve_b += actual_b;
        self.total_shares += shares;
        Ok((actual_a, actual_b, shares))
    }

    /// Remove liquidity by burning LP shares.
    ///
    /// Returns (amount_a_out, amount_b_out).
    pub fn remove_liquidity(&mut self, shares: u128) -> Result<(u128, u128), AmmError> {
        if shares == 0 {
            return Err(AmmError::ZeroAmount);
        }
        if shares > self.total_shares.saturating_sub(MINIMUM_LIQUIDITY) {
            return Err(AmmError::InsufficientLpShares {
                have: self.total_shares.saturating_sub(MINIMUM_LIQUIDITY),
                need: shares,
            });
        }

        let amount_a = shares
            .saturating_mul(self.reserve_a)
            .checked_div(self.total_shares)
            .ok_or(AmmError::Overflow)?;
        let amount_b = shares
            .saturating_mul(self.reserve_b)
            .checked_div(self.total_shares)
            .ok_or(AmmError::Overflow)?;

        self.reserve_a -= amount_a;
        self.reserve_b -= amount_b;
        self.total_shares -= shares;
        Ok((amount_a, amount_b))
    }

    /// Swap token A for token B.
    ///
    /// Returns (amount_b_out, lp_fee_a, protocol_fee_a).
    pub fn swap_a_for_b(
        &mut self,
        amount_a_in: u128,
        min_amount_b_out: u128,
    ) -> Result<SwapResult, AmmError> {
        if amount_a_in == 0 {
            return Err(AmmError::ZeroAmount);
        }
        if self.reserve_a == 0 || self.reserve_b == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }

        // Fee split: total LP_FEE_BPS, PROTOCOL_SHARE_BPS goes to treasury
        let protocol_fee = amount_a_in.saturating_mul(PROTOCOL_SHARE_BPS as u128) / 10_000;
        let lp_fee = amount_a_in.saturating_mul(LP_SHARE_BPS as u128) / 10_000;
        let amount_a_net = amount_a_in - protocol_fee - lp_fee;

        // Constant product: (reserve_a + amount_a_net) * (reserve_b - amount_b_out) = k
        let amount_b_out = self
            .reserve_b
            .saturating_mul(amount_a_net)
            .checked_div(self.reserve_a + amount_a_net)
            .ok_or(AmmError::Overflow)?;

        if amount_b_out == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }

        // Price impact: (amount_b_out / reserve_b) × 10000
        let price_impact_bps = (amount_b_out.saturating_mul(10_000) / self.reserve_b) as u32;
        if price_impact_bps > MAX_PRICE_IMPACT_BPS {
            return Err(AmmError::PriceImpactTooHigh {
                impact_bps: price_impact_bps,
            });
        }

        // Slippage check
        if amount_b_out < min_amount_b_out {
            let actual_slippage = ((min_amount_b_out - amount_b_out).saturating_mul(10_000)
                / min_amount_b_out) as u32;
            return Err(AmmError::SlippageExceeded {
                actual_bps: actual_slippage,
                max_bps: 0, // caller provides min_amount_b_out
            });
        }

        // Update state
        self.reserve_a += amount_a_net + lp_fee; // LP fee stays in pool
        self.reserve_b -= amount_b_out;
        self.fees_collected_a += protocol_fee;
        self.volume_a += amount_a_in;

        Ok(SwapResult {
            amount_in: amount_a_in,
            amount_out: amount_b_out,
            lp_fee,
            protocol_fee,
            price_impact_bps,
        })
    }

    /// Swap token B for token A.
    pub fn swap_b_for_a(
        &mut self,
        amount_b_in: u128,
        min_amount_a_out: u128,
    ) -> Result<SwapResult, AmmError> {
        if amount_b_in == 0 {
            return Err(AmmError::ZeroAmount);
        }
        if self.reserve_a == 0 || self.reserve_b == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }

        let protocol_fee = amount_b_in.saturating_mul(PROTOCOL_SHARE_BPS as u128) / 10_000;
        let lp_fee = amount_b_in.saturating_mul(LP_SHARE_BPS as u128) / 10_000;
        let amount_b_net = amount_b_in - protocol_fee - lp_fee;

        let amount_a_out = self
            .reserve_a
            .saturating_mul(amount_b_net)
            .checked_div(self.reserve_b + amount_b_net)
            .ok_or(AmmError::Overflow)?;

        if amount_a_out == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }

        let price_impact_bps = (amount_a_out.saturating_mul(10_000) / self.reserve_a) as u32;
        if price_impact_bps > MAX_PRICE_IMPACT_BPS {
            return Err(AmmError::PriceImpactTooHigh {
                impact_bps: price_impact_bps,
            });
        }

        if amount_a_out < min_amount_a_out {
            let actual_slippage = ((min_amount_a_out - amount_a_out).saturating_mul(10_000)
                / min_amount_a_out) as u32;
            return Err(AmmError::SlippageExceeded {
                actual_bps: actual_slippage,
                max_bps: 0,
            });
        }

        self.reserve_b += amount_b_net + lp_fee;
        self.reserve_a -= amount_a_out;
        self.fees_collected_b += protocol_fee;
        self.volume_a += amount_a_out; // denominate in A for consistency

        Ok(SwapResult {
            amount_in: amount_b_in,
            amount_out: amount_a_out,
            lp_fee,
            protocol_fee,
            price_impact_bps,
        })
    }

    /// Quote: how much token B would you get for amount_a_in (without executing).
    ///
    /// Uses the exact same fee deduction logic as `swap_a_for_b` to avoid
    /// integer-division rounding divergence between quote and actual swap.
    pub fn quote_a_for_b(&self, amount_a_in: u128) -> u128 {
        if self.reserve_a == 0 || self.reserve_b == 0 || amount_a_in == 0 {
            return 0;
        }
        let protocol_fee = amount_a_in.saturating_mul(PROTOCOL_SHARE_BPS as u128) / 10_000;
        let lp_fee = amount_a_in.saturating_mul(LP_SHARE_BPS as u128) / 10_000;
        let amount_a_net = amount_a_in - protocol_fee - lp_fee;
        self.reserve_b.saturating_mul(amount_a_net) / (self.reserve_a + amount_a_net)
    }

    /// Annual yield for LPs based on fee income vs TVL.
    ///
    /// Annualised from: (fees_collected / reserve_a) × (seconds_per_year / elapsed_secs)
    pub fn lp_apy_bps(&self, elapsed_secs: u64) -> u32 {
        if self.reserve_a == 0 || elapsed_secs == 0 {
            return 0;
        }
        const SECS_PER_YEAR: u64 = 365 * 24 * 3600;
        let fee_rate = self.fees_collected_a.saturating_mul(10_000) / self.reserve_a;
        let annualised = fee_rate.saturating_mul(SECS_PER_YEAR as u128) / elapsed_secs as u128;
        annualised.min(u32::MAX as u128) as u32
    }

    /// Slippage for a given swap size (basis points).
    pub fn slippage_bps(&self, amount_a_in: u128) -> u32 {
        if self.reserve_a == 0 {
            return 10_000;
        }
        let out = self.quote_a_for_b(amount_a_in);
        let ideal = amount_a_in.saturating_mul(self.reserve_b) / self.reserve_a;
        if ideal == 0 {
            return 0;
        }
        let slip = ideal.saturating_sub(out).saturating_mul(10_000) / ideal;
        slip.min(10_000) as u32
    }
}

/// Result of an AMM swap.
#[derive(Debug, Clone, Serialize)]
pub struct SwapResult {
    pub amount_in: u128,
    pub amount_out: u128,
    pub lp_fee: u128,
    pub protocol_fee: u128,
    pub price_impact_bps: u32,
}

// ─── Registry ─────────────────────────────────────────────────────────────────

/// Registry of all AMM pools.
#[derive(Debug, Default)]
pub struct AmmRegistry {
    pools: std::collections::HashMap<PoolId, Pool>,
}

impl AmmRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_create(&mut self, id: PoolId) -> &mut Pool {
        self.pools
            .entry(id.clone())
            .or_insert_with(|| Pool::new(id))
    }

    pub fn get(&self, id: &PoolId) -> Option<&Pool> {
        self.pools.get(id)
    }

    pub fn get_mut(&mut self, id: &PoolId) -> Option<&mut Pool> {
        self.pools.get_mut(id)
    }

    pub fn pool_count(&self) -> usize {
        self.pools.len()
    }

    /// Total value locked across all pools (token A denomination).
    pub fn total_tvl_a(&self) -> u128 {
        self.pools.values().map(|p| p.reserve_a).sum()
    }
}

// ─── Math helpers ──────────────────────────────────────────────────────────────

/// Integer square root (floor).
fn isqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn eth_usdc_pool() -> Pool {
        let id = PoolId::new([0xAA; 20], 1, [0xBB; 20], 1);
        Pool::new(id)
    }

    #[test]
    fn test_add_initial_liquidity() {
        let mut pool = eth_usdc_pool();
        let shares = pool
            .add_initial_liquidity(
                1_000_000_000_000_000_000u128, // 1 ETH
                3_000_000_000u128,             // 3000 USDC (6 decimals)
            )
            .unwrap();
        assert!(shares > 0);
        assert_eq!(pool.reserve_a, 1_000_000_000_000_000_000u128);
        assert_eq!(pool.reserve_b, 3_000_000_000u128);
    }

    #[test]
    fn test_constant_product_invariant() {
        let mut pool = eth_usdc_pool();
        pool.add_initial_liquidity(1_000_000u128, 3_000_000u128)
            .unwrap();
        let k_before = pool.k();

        // After a swap, k should be approximately preserved (with LP fee making it slightly larger)
        pool.swap_a_for_b(10_000, 0).unwrap();
        let k_after = pool.k();

        // k increases slightly because LP fee stays in pool (by design)
        assert!(k_after >= k_before, "k must not decrease after swap");
    }

    #[test]
    fn test_swap_reduces_output_reserve() {
        let mut pool = eth_usdc_pool();
        pool.add_initial_liquidity(1_000_000u128, 3_000_000u128)
            .unwrap();

        let b_before = pool.reserve_b;
        let result = pool.swap_a_for_b(1_000, 0).unwrap();
        assert!(result.amount_out > 0);
        assert_eq!(pool.reserve_b, b_before - result.amount_out);
    }

    #[test]
    fn test_swap_zero_amount_rejected() {
        let mut pool = eth_usdc_pool();
        pool.add_initial_liquidity(1_000_000u128, 3_000_000u128)
            .unwrap();
        assert!(matches!(pool.swap_a_for_b(0, 0), Err(AmmError::ZeroAmount)));
    }

    #[test]
    fn test_slippage_increases_with_trade_size() {
        let id = PoolId::new([0xAA; 20], 1, [0xBB; 20], 1);
        let mut pool = Pool::new(id);
        pool.add_initial_liquidity(1_000_000u128, 3_000_000u128)
            .unwrap();

        let small_slippage = pool.slippage_bps(1_000); // 0.1% of pool
        let large_slippage = pool.slippage_bps(100_000); // 10% of pool
        assert!(
            large_slippage > small_slippage,
            "larger swaps have more slippage: small={small_slippage} bps, large={large_slippage} bps"
        );
    }

    #[test]
    fn test_price_impact_too_high_rejected() {
        let mut pool = eth_usdc_pool();
        pool.add_initial_liquidity(100_000u128, 300_000u128)
            .unwrap();
        // Swap 10% of pool → >5% price impact
        let err = pool.swap_a_for_b(50_000, 0).unwrap_err();
        assert!(matches!(err, AmmError::PriceImpactTooHigh { .. }));
    }

    #[test]
    fn test_add_and_remove_liquidity_roundtrip() {
        let mut pool = eth_usdc_pool();
        let shares = pool
            .add_initial_liquidity(1_000_000u128, 3_000_000u128)
            .unwrap();

        // Remove all user shares (not MINIMUM_LIQUIDITY)
        let (a_out, b_out) = pool.remove_liquidity(shares).unwrap();
        assert!(a_out > 0);
        assert!(b_out > 0);
        // After removing user shares, MINIMUM_LIQUIDITY remains
        assert_eq!(pool.total_shares, MINIMUM_LIQUIDITY);
    }

    #[test]
    fn test_lp_fee_goes_to_protocol() {
        let mut pool = eth_usdc_pool();
        pool.add_initial_liquidity(1_000_000u128, 3_000_000u128)
            .unwrap();

        let result = pool.swap_a_for_b(10_000, 0).unwrap();
        assert!(result.protocol_fee > 0);
        assert!(result.lp_fee > 0);
        // Protocol gets 5 bps of input, LP gets 25 bps
        assert!(result.lp_fee > result.protocol_fee);
        assert_eq!(pool.fees_collected_a, result.protocol_fee);
    }

    #[test]
    fn test_quote_matches_swap() {
        let mut pool = eth_usdc_pool();
        pool.add_initial_liquidity(1_000_000u128, 3_000_000u128)
            .unwrap();

        let quoted = pool.quote_a_for_b(1_000);
        let result = pool.swap_a_for_b(1_000, 0).unwrap();
        // Quote and swap should be within 1 unit (rounding)
        assert!(
            (quoted as i128 - result.amount_out as i128).abs() <= 1,
            "quote={quoted} swap={}",
            result.amount_out
        );
    }

    #[test]
    fn test_canonical_pool_id_ordering() {
        let a = [0xAA; 20];
        let b = [0xBB; 20];
        let id1 = PoolId::new(a, 1, b, 1);
        let id2 = PoolId::new(b, 1, a, 1); // swapped order
        assert_eq!(id1, id2, "canonical ordering must make IDs equal");
    }

    #[test]
    fn test_registry_get_or_create() {
        let mut registry = AmmRegistry::new();
        let id = PoolId::new([0xAA; 20], 1, [0xBB; 20], 1);
        let pool = registry.get_or_create(id.clone());
        pool.add_initial_liquidity(1_000_000, 3_000_000).unwrap();

        assert_eq!(registry.pool_count(), 1);
        assert_eq!(registry.total_tvl_a(), 1_000_000);
    }

    #[test]
    fn test_spot_price() {
        let mut pool = eth_usdc_pool();
        // 1 ETH = 3000 USDC: price = 3000/1 = 3000 (scaled × 1e6 = 3_000_000_000)
        pool.add_initial_liquidity(1_000_000u128, 3_000_000_000u128)
            .unwrap();
        let price = pool.spot_price_a_in_b_e6();
        // 3_000_000_000 * 1_000_000 / 1_000_000 = 3_000_000_000
        assert_eq!(price, 3_000_000_000u128);
    }
}
