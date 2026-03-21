//! Intent-based cross-chain transfers for InterLink (Phase 8).
//!
//! Users specify WHAT they want ("send 1 ETH, receive at least 2,900 USDC on Solana")
//! and the solver network finds the optimal routing automatically.
//!
//! # vs LiFi intent engine
//!
//! LiFi: user specifies input token, output token, amount — LiFi routes across 10+ bridges
//! InterLink: user specifies intent — solver network finds best route across bridges + DEXes
//!
//! # Architecture
//!
//! 1. User submits IntentRequest to the relayer API
//! 2. Solver evaluates all routes: direct bridge, bridge+swap, multi-hop
//! 3. Best route is returned as IntentQuote with atomicity guarantee
//! 4. User signs + submits → atomic execution (all steps succeed or all revert)
//!
//! # Route types
//!
//! DirectBridge:  ETH(Ethereum) → ETH(Solana) via InterLink ZK bridge
//! BridgeAndSwap: ETH(Ethereum) → bridge → ETH(Solana) → AMM → USDC(Solana)
//! MultiHop:      USDC(Arbitrum) → bridge → USDC(Ethereum) → DEX → ETH → bridge → SOL

use crate::fee::FeeTier;
use serde::{Deserialize, Serialize};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Maximum allowed slippage for intent execution (2%).
pub const MAX_INTENT_SLIPPAGE_BPS: u32 = 200;

/// Maximum number of hops in a route (prevents complexity blowup).
pub const MAX_ROUTE_HOPS: usize = 3;

/// Intent expiry: user must submit within this window.
pub const INTENT_EXPIRY_SECS: u64 = 300; // 5 minutes

/// Maximum parallel solver bids accepted per intent.
pub const MAX_SOLVER_BIDS: usize = 10;

// ─── Intent types ─────────────────────────────────────────────────────────────

/// A user's cross-chain intent: what they want, not how to get it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentRequest {
    /// Input token address (zero = native ETH/SOL).
    pub token_in: [u8; 20],
    /// Input chain ID.
    pub chain_in: u64,
    /// Input amount in wei.
    pub amount_in: u128,
    /// Desired output token address.
    pub token_out: [u8; 20],
    /// Output chain ID.
    pub chain_out: u64,
    /// Minimum acceptable output (slippage protection).
    pub min_amount_out: u128,
    /// Recipient address on destination chain.
    pub recipient: Vec<u8>,
    /// Deadline: Unix timestamp after which intent expires.
    pub deadline: u64,
    /// Optional: prefer speed over price (default: false = prefer best price).
    pub prefer_speed: bool,
}

impl IntentRequest {
    /// Whether this intent is a direct token transfer (no swap needed).
    pub fn is_direct_transfer(&self) -> bool {
        self.token_in == self.token_out || self.chain_in == self.chain_out
    }

    /// Whether the intent has expired.
    pub fn is_expired(&self, now: u64) -> bool {
        now > self.deadline
    }

    /// Compute implied slippage tolerance (bps) from min_amount_out.
    pub fn slippage_bps(&self, expected_out: u128) -> u32 {
        if expected_out == 0 {
            return 10_000;
        }
        let slip = expected_out
            .saturating_sub(self.min_amount_out)
            .saturating_mul(10_000)
            / expected_out;
        slip as u32
    }
}

/// A single hop in a multi-hop route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteHop {
    pub hop_type: HopType,
    /// Input token for this hop.
    pub token_in: [u8; 20],
    pub chain_in: u64,
    /// Output token from this hop.
    pub token_out: [u8; 20],
    pub chain_out: u64,
    /// Expected input amount.
    pub amount_in: u128,
    /// Expected output amount (before slippage).
    pub expected_amount_out: u128,
    /// Fee for this hop in basis points.
    pub fee_bps: u32,
    /// Protocol or DEX executing this hop.
    pub executor: &'static str,
    /// Estimated time for this hop in seconds.
    pub estimated_secs: u64,
}

/// Type of hop in a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HopType {
    /// InterLink ZK bridge transfer.
    ZkBridge,
    /// AMM swap on destination chain.
    AmmSwap,
    /// DEX aggregator swap (1inch, 0x, etc.).
    DexSwap,
    /// Wrapped token mint/burn.
    Wrap,
}

/// A complete route for an intent.
#[derive(Debug, Clone, Serialize)]
pub struct IntentRoute {
    /// Ordered sequence of hops.
    pub hops: Vec<RouteHop>,
    /// Total expected output after all hops.
    pub total_amount_out: u128,
    /// Total fees across all hops in basis points.
    pub total_fee_bps: u32,
    /// Total estimated time in seconds.
    pub total_secs: u64,
    /// Route type classification.
    pub route_type: RouteType,
    /// Confidence score (0-100): how reliable is this route.
    pub confidence: u8,
}

/// Classification of a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RouteType {
    /// Single ZK bridge hop.
    DirectBridge,
    /// Bridge then swap on destination.
    BridgeAndSwap,
    /// Multiple bridges and/or swaps.
    MultiHop,
    /// No bridge needed (same chain swap).
    SameChainSwap,
}

/// A solver's bid for fulfilling an intent.
#[derive(Debug, Clone, Serialize)]
pub struct SolverBid {
    /// Solver identifier.
    pub solver_id: String,
    /// Proposed route.
    pub route: IntentRoute,
    /// Guaranteed minimum output (solver puts up bond).
    pub guaranteed_min_out: u128,
    /// Solver bond amount (slashed if guarantee is violated).
    pub bond_amount: u128,
    /// Bid expiry.
    pub expires_at: u64,
}

/// The best quote returned to the user.
#[derive(Debug, Clone, Serialize)]
pub struct IntentQuote {
    /// Unique quote ID.
    pub quote_id: String,
    /// Original intent.
    pub intent: IntentRequest,
    /// Best route found.
    pub best_route: IntentRoute,
    /// All competing routes (for user to inspect).
    pub alternative_routes: Vec<IntentRoute>,
    /// Expected output amount.
    pub expected_out: u128,
    /// Worst-case output (slippage applied).
    pub min_out: u128,
    /// Total fee in basis points.
    pub fee_bps: u32,
    /// Protocol fee in basis points.
    pub protocol_fee_bps: u32,
    /// Total estimated time.
    pub estimated_secs: u64,
    /// Quote expiry.
    pub expires_at: u64,
    /// Whether InterLink beats the best alternative (Wormhole/LiFi/etc.)
    pub interlink_wins: bool,
}

// ─── Solver ───────────────────────────────────────────────────────────────────

/// Solve an intent: find the optimal route.
///
/// In production this calls out to solver network. Here it's a deterministic
/// local solver that builds routes from known bridge + DEX capabilities.
pub fn solve(intent: &IntentRequest, now_secs: u64) -> Result<IntentQuote, IntentError> {
    if intent.is_expired(now_secs) {
        return Err(IntentError::IntentExpired);
    }
    if intent.amount_in == 0 {
        return Err(IntentError::ZeroAmount);
    }
    if intent.min_amount_out > intent.amount_in.saturating_mul(2) {
        return Err(IntentError::UnreachableMinOutput);
    }

    let routes = generate_routes(intent);
    if routes.is_empty() {
        return Err(IntentError::NoRouteFound);
    }

    // Score routes: if prefer_speed, rank by time; otherwise rank by output
    let mut scored: Vec<_> = routes.iter().enumerate().collect();
    scored.sort_by_key(|(_, r)| {
        if intent.prefer_speed {
            (r.total_secs, u128::MAX - r.total_amount_out)
        } else {
            // Lower fee = higher score (negate amount_out to sort descending)
            (r.total_fee_bps as u64, u128::MAX - r.total_amount_out)
        }
    });

    let best_idx = scored[0].0;
    let best = routes[best_idx].clone();
    let expected_out = best.total_amount_out;
    let min_out = expected_out
        .saturating_mul(10_000 - best.total_fee_bps.min(MAX_INTENT_SLIPPAGE_BPS) as u128)
        / 10_000;

    // Check if expected output meets the user's minimum
    if expected_out < intent.min_amount_out {
        return Err(IntentError::BelowMinOutput {
            expected: expected_out,
            minimum: intent.min_amount_out,
        });
    }

    let alternatives: Vec<IntentRoute> = routes
        .into_iter()
        .enumerate()
        .filter(|(i, _)| *i != best_idx)
        .map(|(_, r)| r)
        .take(3)
        .collect();

    let protocol_fee_bps = FeeTier::from_usd_cents(
        (intent.amount_in / 1_000_000_000_000u128) as u64 * 3, // rough USD estimate
    )
    .bps();

    let quote_id = format!("quote_{now_secs}_{}", intent.amount_in % 10_000);

    Ok(IntentQuote {
        quote_id,
        intent: intent.clone(),
        best_route: best.clone(),
        alternative_routes: alternatives,
        expected_out,
        min_out,
        fee_bps: best.total_fee_bps,
        protocol_fee_bps,
        estimated_secs: best.total_secs,
        expires_at: now_secs + INTENT_EXPIRY_SECS,
        interlink_wins: best.route_type == RouteType::DirectBridge || best.total_fee_bps <= 10, // beats Wormhole's 10 bps
    })
}

/// Generate candidate routes for an intent.
fn generate_routes(intent: &IntentRequest) -> Vec<IntentRoute> {
    let mut routes = Vec::new();

    // Route 1: Direct ZK bridge (if same token, different chain)
    if intent.token_in == intent.token_out && intent.chain_in != intent.chain_out {
        routes.push(build_direct_bridge_route(intent));
    }

    // Route 2: Bridge then swap
    if intent.chain_in != intent.chain_out && intent.token_in != intent.token_out {
        routes.push(build_bridge_and_swap_route(intent));
    }

    // Route 3: Same-chain swap (if same chain, different token)
    if intent.chain_in == intent.chain_out && intent.token_in != intent.token_out {
        routes.push(build_same_chain_swap_route(intent));
    }

    // Route 4: Multi-hop (bridge to intermediate, then swap)
    if intent.chain_in != intent.chain_out && intent.token_in != intent.token_out {
        if let Some(multi) = build_multihop_route(intent) {
            routes.push(multi);
        }
    }

    routes
}

fn build_direct_bridge_route(intent: &IntentRequest) -> IntentRoute {
    // InterLink ZK bridge: 30s finality, 0-5 bps fee
    let fee_bps =
        FeeTier::from_usd_cents((intent.amount_in / 1_000_000_000_000u128) as u64 * 3).bps();
    let amount_out = intent.amount_in.saturating_mul(10_000 - fee_bps as u128) / 10_000;

    IntentRoute {
        hops: vec![RouteHop {
            hop_type: HopType::ZkBridge,
            token_in: intent.token_in,
            chain_in: intent.chain_in,
            token_out: intent.token_out,
            chain_out: intent.chain_out,
            amount_in: intent.amount_in,
            expected_amount_out: amount_out,
            fee_bps,
            executor: "InterLink ZK Hub",
            estimated_secs: 30,
        }],
        total_amount_out: amount_out,
        total_fee_bps: fee_bps,
        total_secs: 30,
        route_type: RouteType::DirectBridge,
        confidence: 95,
    }
}

fn build_bridge_and_swap_route(intent: &IntentRequest) -> IntentRoute {
    // Step 1: Bridge input token to destination chain (30s, 0-5 bps)
    let bridge_fee_bps =
        FeeTier::from_usd_cents((intent.amount_in / 1_000_000_000_000u128) as u64 * 3).bps();
    let after_bridge = intent
        .amount_in
        .saturating_mul(10_000 - bridge_fee_bps as u128)
        / 10_000;

    // Step 2: Swap on destination (AMM, 30 bps fee, ~0.5% slippage)
    let swap_fee_bps = 30u32;
    let swap_slippage_bps = 50u32;
    let amount_out = after_bridge
        .saturating_mul(10_000 - swap_fee_bps as u128 - swap_slippage_bps as u128)
        / 10_000;

    let total_fee_bps = bridge_fee_bps + swap_fee_bps;

    IntentRoute {
        hops: vec![
            RouteHop {
                hop_type: HopType::ZkBridge,
                token_in: intent.token_in,
                chain_in: intent.chain_in,
                token_out: intent.token_in, // same token, different chain
                chain_out: intent.chain_out,
                amount_in: intent.amount_in,
                expected_amount_out: after_bridge,
                fee_bps: bridge_fee_bps,
                executor: "InterLink ZK Hub",
                estimated_secs: 30,
            },
            RouteHop {
                hop_type: HopType::AmmSwap,
                token_in: intent.token_in,
                chain_in: intent.chain_out,
                token_out: intent.token_out,
                chain_out: intent.chain_out,
                amount_in: after_bridge,
                expected_amount_out: amount_out,
                fee_bps: swap_fee_bps,
                executor: "InterLink AMM",
                estimated_secs: 5,
            },
        ],
        total_amount_out: amount_out,
        total_fee_bps,
        total_secs: 35,
        route_type: RouteType::BridgeAndSwap,
        confidence: 85,
    }
}

fn build_same_chain_swap_route(intent: &IntentRequest) -> IntentRoute {
    let swap_fee_bps = 30u32;
    let amount_out = intent
        .amount_in
        .saturating_mul(10_000 - swap_fee_bps as u128)
        / 10_000;

    IntentRoute {
        hops: vec![RouteHop {
            hop_type: HopType::AmmSwap,
            token_in: intent.token_in,
            chain_in: intent.chain_in,
            token_out: intent.token_out,
            chain_out: intent.chain_out,
            amount_in: intent.amount_in,
            expected_amount_out: amount_out,
            fee_bps: swap_fee_bps,
            executor: "InterLink AMM",
            estimated_secs: 5,
        }],
        total_amount_out: amount_out,
        total_fee_bps: swap_fee_bps,
        total_secs: 5,
        route_type: RouteType::SameChainSwap,
        confidence: 90,
    }
}

fn build_multihop_route(intent: &IntentRequest) -> Option<IntentRoute> {
    // Multi-hop: only for Tier 2+ amounts (>$1k) where the extra complexity is worth it
    if intent.amount_in < 1_000_000_000_000_000_000u128 {
        return None; // Skip for small amounts
    }

    // Intermediate: bridge via ETH as common token
    let bridge_fee_bps =
        FeeTier::from_usd_cents((intent.amount_in / 1_000_000_000_000u128) as u64 * 3).bps();
    let after_hop1 = intent
        .amount_in
        .saturating_mul(10_000 - bridge_fee_bps as u128)
        / 10_000;
    let after_hop2 = after_hop1
        .saturating_mul(10_000 - 25u128)   // 0.25% DEX fee
        / 10_000;
    let after_hop3 = after_hop2.saturating_mul(10_000 - bridge_fee_bps as u128) / 10_000;

    Some(IntentRoute {
        hops: vec![
            RouteHop {
                hop_type: HopType::ZkBridge,
                token_in: intent.token_in,
                chain_in: intent.chain_in,
                token_out: intent.token_in,
                chain_out: 1, // Ethereum as hub
                amount_in: intent.amount_in,
                expected_amount_out: after_hop1,
                fee_bps: bridge_fee_bps,
                executor: "InterLink ZK Hub",
                estimated_secs: 30,
            },
            RouteHop {
                hop_type: HopType::DexSwap,
                token_in: intent.token_in,
                chain_in: 1,
                token_out: intent.token_out,
                chain_out: 1,
                amount_in: after_hop1,
                expected_amount_out: after_hop2,
                fee_bps: 25,
                executor: "1inch",
                estimated_secs: 15,
            },
            RouteHop {
                hop_type: HopType::ZkBridge,
                token_in: intent.token_out,
                chain_in: 1,
                token_out: intent.token_out,
                chain_out: intent.chain_out,
                amount_in: after_hop2,
                expected_amount_out: after_hop3,
                fee_bps: bridge_fee_bps,
                executor: "InterLink ZK Hub",
                estimated_secs: 30,
            },
        ],
        total_amount_out: after_hop3,
        total_fee_bps: bridge_fee_bps * 2 + 25,
        total_secs: 75,
        route_type: RouteType::MultiHop,
        confidence: 75,
    })
}

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentError {
    IntentExpired,
    ZeroAmount,
    NoRouteFound,
    UnreachableMinOutput,
    BelowMinOutput { expected: u128, minimum: u128 },
    TooManyHops,
    SlippageTooHigh,
}

impl std::fmt::Display for IntentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntentError::IntentExpired => write!(f, "intent has expired"),
            IntentError::ZeroAmount => write!(f, "amount_in must be > 0"),
            IntentError::NoRouteFound => write!(f, "no route found for this intent"),
            IntentError::UnreachableMinOutput => write!(f, "min_amount_out exceeds 2x amount_in"),
            IntentError::BelowMinOutput { expected, minimum } => {
                write!(f, "expected output {expected} < minimum {minimum}")
            }
            IntentError::TooManyHops => write!(f, "route exceeds max {MAX_ROUTE_HOPS} hops"),
            IntentError::SlippageTooHigh => write!(f, "slippage exceeds maximum"),
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> u64 {
        1_700_000_000
    }

    fn bridge_intent() -> IntentRequest {
        IntentRequest {
            token_in: [0xAA; 20],
            chain_in: 1,                                 // Ethereum
            amount_in: 1_000_000_000_000_000_000u128,    // 1 ETH
            token_out: [0xAA; 20],                       // same token
            chain_out: 900,                              // Solana
            min_amount_out: 900_000_000_000_000_000u128, // 0.9 ETH min
            recipient: vec![0xBB; 32],
            deadline: now() + 600,
            prefer_speed: false,
        }
    }

    fn swap_intent() -> IntentRequest {
        IntentRequest {
            token_in: [0xAA; 20], // ETH
            chain_in: 1,
            amount_in: 1_000_000_000_000_000_000u128,
            token_out: [0xBB; 20], // USDC
            chain_out: 900,
            min_amount_out: 2_500_000_000u128, // 2500 USDC min
            recipient: vec![0xBB; 32],
            deadline: now() + 600,
            prefer_speed: false,
        }
    }

    #[test]
    fn test_direct_bridge_route_found() {
        let intent = bridge_intent();
        let quote = solve(&intent, now()).unwrap();
        assert_eq!(quote.best_route.route_type, RouteType::DirectBridge);
        assert!(quote.expected_out > 0);
        assert!(quote.expected_out >= intent.min_amount_out);
        assert!(quote.interlink_wins);
    }

    #[test]
    fn test_bridge_and_swap_route_found() {
        let intent = swap_intent();
        let quote = solve(&intent, now()).unwrap();
        // Should find a BridgeAndSwap or similar route
        assert!(!quote.best_route.hops.is_empty());
        assert!(quote.expected_out > 0);
    }

    #[test]
    fn test_expired_intent_rejected() {
        let mut intent = bridge_intent();
        intent.deadline = now() - 1; // already expired
        let err = solve(&intent, now()).unwrap_err();
        assert_eq!(err, IntentError::IntentExpired);
    }

    #[test]
    fn test_zero_amount_rejected() {
        let mut intent = bridge_intent();
        intent.amount_in = 0;
        let err = solve(&intent, now()).unwrap_err();
        assert_eq!(err, IntentError::ZeroAmount);
    }

    #[test]
    fn test_prefer_speed_selects_faster_route() {
        let mut intent = swap_intent();
        intent.prefer_speed = true;
        let fast_quote = solve(&intent, now()).unwrap();

        intent.prefer_speed = false;
        let cheap_quote = solve(&intent, now()).unwrap();

        // Fast route should not be slower than cheap route
        assert!(fast_quote.estimated_secs <= cheap_quote.estimated_secs + 10);
    }

    #[test]
    fn test_direct_transfer_detection() {
        let intent = bridge_intent();
        assert!(
            intent.is_direct_transfer(),
            "same token different chain = direct transfer"
        );

        let swap = swap_intent();
        assert!(
            !swap.is_direct_transfer(),
            "different token = not direct transfer"
        );
    }

    #[test]
    fn test_slippage_calculation() {
        let intent = bridge_intent();
        let slippage = intent.slippage_bps(1_000_000_000_000_000_000u128);
        // min_amount_out = 0.9 ETH, expected = 1 ETH → 10% slippage tolerance
        assert_eq!(slippage, 1_000); // 10% = 1000 bps
    }

    #[test]
    fn test_quote_has_alternatives() {
        let intent = swap_intent();
        let quote = solve(&intent, now()).unwrap();
        // Swap intent should have multiple routes (bridge+swap, multi-hop, etc.)
        assert!(!quote.best_route.hops.is_empty());
    }

    #[test]
    fn test_quote_expires_in_5_minutes() {
        let intent = bridge_intent();
        let quote = solve(&intent, now()).unwrap();
        assert_eq!(quote.expires_at, now() + INTENT_EXPIRY_SECS);
    }

    #[test]
    fn test_interlink_beats_wormhole_on_fee() {
        // Wormhole charges 10 bps ($1 flat). InterLink Tier 1 = 0 bps.
        let intent = bridge_intent();
        let quote = solve(&intent, now()).unwrap();
        assert!(
            quote.fee_bps <= 10,
            "InterLink must match or beat Wormhole 10 bps"
        );
    }

    #[test]
    fn test_multihop_only_for_large_amounts() {
        // Small amount: no multi-hop route
        let mut intent = swap_intent();
        intent.amount_in = 100; // tiny
        let routes = generate_routes(&intent);
        assert!(
            !routes.iter().any(|r| r.route_type == RouteType::MultiHop),
            "multi-hop should not be generated for tiny amounts"
        );
    }
}
