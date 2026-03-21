//! Dynamic fee tier calculator for InterLink bridge transfers.
//!
//! Fee structure designed to beat all major competitors:
//!   Tier 1 ($0–$1k):      0.00%  — Wormhole charges $1–20 flat; we charge nothing
//!   Tier 2 ($1k–$100k):   0.05%  — Wormhole 0.1–0.2%, Stargate 0.5–5%; we beat both
//!   Tier 3 ($100k–$10M):  0.01%  — Across 0.25–1%; we beat by 25–100x
//!   Tier 4 (>$10M):       0.00%  — OTC / direct negotiation
//!
//! Basis points (bps): 1 bps = 0.01%.  100 bps = 1%.

/// Fee tier for a transfer based on USD value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeTier {
    /// $0–$1,000: zero fee. LP yield on larger transfers subsidises small txs.
    /// Wormhole charges $1–20 per tx at this size — we charge nothing.
    Zero,
    /// $1,000–$100,000: 0.05% (5 bps).
    /// Beats Wormhole (0.1–0.2%) and Stargate (0.5–5%) by 2–100x.
    Standard,
    /// $100,000–$10,000,000: 0.01% (1 bps).
    /// Beats Across (0.25–1%) by 25–100x.
    Institutional,
    /// >$10,000,000: 0% — negotiate directly; bridge fee waived.
    OTC,
}

impl FeeTier {
    /// Fee in basis points (1 bps = 0.01%).
    pub fn bps(&self) -> u32 {
        match self {
            FeeTier::Zero => 0,
            FeeTier::Standard => 5,      // 0.05%
            FeeTier::Institutional => 1, // 0.01%
            FeeTier::OTC => 0,
        }
    }

    /// Human-readable description for API responses and logs.
    pub fn describe(&self) -> &'static str {
        match self {
            FeeTier::Zero => "0% (Tier 1: <$1k, vs Wormhole $1-20/tx)",
            FeeTier::Standard => "0.05% (Tier 2: $1k-$100k, vs Wormhole 0.1-0.2%)",
            FeeTier::Institutional => "0.01% (Tier 3: $100k-$10M, vs Across 0.25-1%)",
            FeeTier::OTC => "0% (Tier 4: >$10M, OTC negotiated)",
        }
    }

    /// Classify a transfer by its USD value (in cents, so $1.00 = 100).
    pub fn from_usd_cents(usd_cents: u64) -> Self {
        match usd_cents {
            0..=99_999 => FeeTier::Zero,                        // $0–$999.99
            100_000..=9_999_999 => FeeTier::Standard,           // $1,000–$99,999.99
            10_000_000..=999_999_999 => FeeTier::Institutional, // $100k–$9,999,999.99
            _ => FeeTier::OTC,                                  // $10M+
        }
    }
}

/// Calculate the bridge fee for a transfer.
///
/// # Arguments
/// - `amount`: token amount in smallest denomination (wei, lamports, etc.)
/// - `usd_cents`: current USD value of `amount` in cents (100 = $1.00)
///
/// # Returns
/// Fee in the same denomination as `amount`. Returns 0 for Tier 1 and OTC.
pub fn calculate_fee(amount: u128, usd_cents: u64) -> u128 {
    let tier = FeeTier::from_usd_cents(usd_cents);
    let bps = tier.bps() as u128;
    if bps == 0 {
        return 0;
    }
    // saturating_mul prevents overflow on extreme amounts (u128::MAX * 5 would overflow).
    // Round down (conservative — never over-charge).
    amount.saturating_mul(bps) / 10_000
}

/// Return the net amount a recipient receives after fees.
pub fn amount_after_fee(amount: u128, usd_cents: u64) -> u128 {
    amount.saturating_sub(calculate_fee(amount, usd_cents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_classification() {
        assert_eq!(FeeTier::from_usd_cents(0), FeeTier::Zero);
        assert_eq!(FeeTier::from_usd_cents(99_999), FeeTier::Zero); // $999.99
        assert_eq!(FeeTier::from_usd_cents(100_000), FeeTier::Standard); // $1,000.00
        assert_eq!(FeeTier::from_usd_cents(9_999_999), FeeTier::Standard); // $99,999.99
        assert_eq!(FeeTier::from_usd_cents(10_000_000), FeeTier::Institutional); // $100k
        assert_eq!(FeeTier::from_usd_cents(999_999_999), FeeTier::Institutional); // $9.99M
        assert_eq!(FeeTier::from_usd_cents(1_000_000_000), FeeTier::OTC); // $10M+
    }

    #[test]
    fn test_zero_fee_tier() {
        // $500 transfer — no fee regardless of amount
        let fee = calculate_fee(1_000_000_000_000_000_000u128, 50_000);
        assert_eq!(fee, 0);
    }

    #[test]
    fn test_standard_fee_0_05_percent() {
        // $10,000 transfer of 1 ETH (1e18 wei): fee should be 0.05% = 5e14 wei
        // 0.05% = 5 bps = 5/10_000 => 1e18 * 5 / 10_000 = 5e14
        let amount = 1_000_000_000_000_000_000u128; // 1 ETH in wei
        let usd_cents = 1_000_000; // $10,000.00
        let fee = calculate_fee(amount, usd_cents);
        // 0.05% of 1e18 = 5e14
        assert_eq!(fee, 500_000_000_000_000);
    }

    #[test]
    fn test_institutional_fee_0_01_percent() {
        // $1M transfer: fee should be 0.01% = 1 bps
        let amount = 1_000_000_000_000_000_000_000u128; // large amount
        let usd_cents = 100_000_000; // $1M
        let fee = calculate_fee(amount, usd_cents);
        assert_eq!(fee, amount / 10_000); // 1 bps
    }

    #[test]
    fn test_otc_zero_fee() {
        let fee = calculate_fee(u128::MAX, 1_000_000_001);
        assert_eq!(fee, 0);
    }

    #[test]
    fn test_amount_after_fee() {
        let amount = 1_000_000u128;
        // Tier 1: no fee
        assert_eq!(amount_after_fee(amount, 50_000), amount);
        // Tier 2: 0.05% fee = 500 units deducted (1_000_000 * 5 / 10_000 = 500)
        assert_eq!(amount_after_fee(amount, 100_000), 999_500);
    }

    #[test]
    fn test_competitive_advantage_vs_wormhole() {
        // $100 transfer: Wormhole charges $1-20 flat. InterLink charges $0.
        // $5,000 transfer: Wormhole charges ~0.1-0.2%. InterLink charges 0.05%.
        let small_fee = calculate_fee(100_000_000u128, 10_000); // $100
        let medium_fee_bps = FeeTier::Standard.bps(); // 5 bps
        let wormhole_min_bps = 10u32; // 0.1%

        assert_eq!(small_fee, 0, "Tier 1 must be free");
        assert!(
            medium_fee_bps < wormhole_min_bps,
            "Tier 2 must beat Wormhole minimum"
        );
    }
}
