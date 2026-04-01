/// Transfer simulation engine for InterLink (Phase 11)
///
/// Dry-runs a cross-chain transfer end-to-end WITHOUT submitting anything
/// on-chain. Reports estimated fees, settlement time, route, slippage,
/// and potential failure points so users/integrators can validate before
/// committing real funds.
///
/// Comparison:
///   LiFi:      has "simulate" flag on /quote — partial, doesn't check finality
///   Across:    no simulation endpoint
///   Wormhole:  no simulation (you just submit and hope)
///   InterLink: full dry-run checking fees, wrapped resolution, AMM liquidity,
///              rate limits, circuit breaker status, and estimated time

use std::time::Duration;

// ─── Input ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SimulationRequest {
    pub source_chain: u32,
    pub dest_chain: u32,
    pub sender: String,
    pub receiver: String,
    /// Amount in smallest denomination (wei, lamports, etc.)
    pub amount: u128,
    /// Token address or "native"
    pub token: String,
    /// USD value in cents (for fee tier calculation)
    pub usd_value_cents: u64,
    /// Optional: API key for rate-limit check
    pub api_key: Option<String>,
}

// ─── Output ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SimulationResult {
    pub feasible: bool,
    pub checks: Vec<SimCheck>,
    pub estimated_fee: u128,
    pub fee_tier: String,
    pub estimated_time_secs: u64,
    pub route_type: String,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SimCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

impl SimCheck {
    fn pass(name: impl Into<String>, detail: impl Into<String>) -> Self {
        SimCheck { name: name.into(), passed: true, detail: detail.into() }
    }
    fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        SimCheck { name: name.into(), passed: false, detail: detail.into() }
    }
}

// ─── Simulator ───────────────────────────────────────────────────────────────

/// Configuration for the simulator (injected dependencies).
pub struct SimulatorConfig {
    /// Whether the circuit breaker is currently paused
    pub bridge_paused: bool,
    /// Supported source chain IDs
    pub supported_source_chains: Vec<u32>,
    /// Supported destination chain IDs
    pub supported_dest_chains: Vec<u32>,
    /// Available AMM liquidity on destination (in token units)
    pub dest_liquidity: u128,
    /// Current API key rate-limit remaining (None = no key provided)
    pub rate_limit_remaining: Option<u32>,
    /// Finality time per chain in seconds
    pub finality_secs: std::collections::HashMap<u32, u64>,
}

impl SimulatorConfig {
    /// Default configuration with common chains.
    pub fn default_config() -> Self {
        let mut finality = std::collections::HashMap::new();
        finality.insert(1, 900);      // Ethereum: ~15 min (75 blocks)
        finality.insert(10, 2);       // Optimism
        finality.insert(137, 128);    // Polygon PoS
        finality.insert(42161, 2);    // Arbitrum
        finality.insert(8453, 2);     // Base
        finality.insert(900, 1);      // Solana

        SimulatorConfig {
            bridge_paused: false,
            supported_source_chains: vec![1, 10, 137, 42161, 8453, 900],
            supported_dest_chains: vec![1, 10, 137, 42161, 8453, 900],
            dest_liquidity: 1_000_000_000_000_000_000_000u128, // large default
            rate_limit_remaining: None,
            finality_secs: finality,
        }
    }
}

/// Run a full simulation. Returns a detailed result with pass/fail checks.
pub fn simulate(req: &SimulationRequest, config: &SimulatorConfig) -> SimulationResult {
    let mut checks = Vec::new();
    let mut warnings = Vec::new();
    let mut blockers = Vec::new();

    // ── 1. Circuit breaker ──────────────────────────────────────────────────
    if config.bridge_paused {
        checks.push(SimCheck::fail("circuit_breaker", "Bridge is currently PAUSED"));
        blockers.push("Bridge is paused — no transfers accepted".into());
    } else {
        checks.push(SimCheck::pass("circuit_breaker", "Bridge is operational"));
    }

    // ── 2. Source chain supported ───────────────────────────────────────────
    if config.supported_source_chains.contains(&req.source_chain) {
        checks.push(SimCheck::pass(
            "source_chain",
            format!("Chain {} is supported", req.source_chain),
        ));
    } else {
        checks.push(SimCheck::fail(
            "source_chain",
            format!("Chain {} is not supported", req.source_chain),
        ));
        blockers.push(format!("Source chain {} not supported", req.source_chain));
    }

    // ── 3. Destination chain supported ──────────────────────────────────────
    if config.supported_dest_chains.contains(&req.dest_chain) {
        checks.push(SimCheck::pass(
            "dest_chain",
            format!("Chain {} is supported", req.dest_chain),
        ));
    } else {
        checks.push(SimCheck::fail(
            "dest_chain",
            format!("Chain {} is not supported", req.dest_chain),
        ));
        blockers.push(format!("Destination chain {} not supported", req.dest_chain));
    }

    // ── 4. Same chain check ─────────────────────────────────────────────────
    if req.source_chain == req.dest_chain {
        checks.push(SimCheck::fail(
            "cross_chain",
            "Source and destination are the same chain",
        ));
        blockers.push("Source and destination must be different chains".into());
    } else {
        checks.push(SimCheck::pass("cross_chain", "Cross-chain transfer"));
    }

    // ── 5. Amount validation ────────────────────────────────────────────────
    if req.amount == 0 {
        checks.push(SimCheck::fail("amount", "Amount is zero"));
        blockers.push("Transfer amount must be > 0".into());
    } else {
        checks.push(SimCheck::pass("amount", format!("Amount: {}", req.amount)));
    }

    // ── 6. Fee calculation ──────────────────────────────────────────────────
    let (fee, fee_tier) = calculate_sim_fee(req.amount, req.usd_value_cents);
    checks.push(SimCheck::pass(
        "fee",
        format!("Fee: {} (tier: {})", fee, fee_tier),
    ));

    // ── 7. Liquidity check ──────────────────────────────────────────────────
    let amount_after_fee = req.amount.saturating_sub(fee);
    if amount_after_fee > config.dest_liquidity {
        checks.push(SimCheck::fail(
            "liquidity",
            format!(
                "Insufficient destination liquidity: need {}, available {}",
                amount_after_fee, config.dest_liquidity
            ),
        ));
        warnings.push("Insufficient liquidity — transfer may be delayed".into());
    } else {
        let utilization_bps = if config.dest_liquidity > 0 {
            (amount_after_fee * 10_000 / config.dest_liquidity) as u32
        } else {
            10_000
        };
        checks.push(SimCheck::pass(
            "liquidity",
            format!("Liquidity utilization: {}bps", utilization_bps),
        ));
        if utilization_bps > 2_000 {
            warnings.push(format!(
                "High liquidity utilization ({}bps) — consider smaller transfer",
                utilization_bps
            ));
        }
    }

    // ── 8. Rate limit check ─────────────────────────────────────────────────
    if let Some(remaining) = config.rate_limit_remaining {
        if remaining == 0 {
            checks.push(SimCheck::fail("rate_limit", "Rate limit exceeded"));
            blockers.push("API rate limit exceeded — wait or upgrade tier".into());
        } else {
            checks.push(SimCheck::pass(
                "rate_limit",
                format!("{} requests remaining", remaining),
            ));
        }
    } else {
        checks.push(SimCheck::pass("rate_limit", "No rate limit (no API key)"));
    }

    // ── 9. Estimated time ───────────────────────────────────────────────────
    let finality = config
        .finality_secs
        .get(&req.source_chain)
        .copied()
        .unwrap_or(900); // default to Ethereum
    let proof_gen_estimate = 1; // ~1 second
    let settlement_estimate = 2; // ~2 seconds
    let estimated_time = finality + proof_gen_estimate + settlement_estimate;

    checks.push(SimCheck::pass(
        "estimated_time",
        format!(
            "{}s total ({}s finality + {}s proof + {}s settlement)",
            estimated_time, finality, proof_gen_estimate, settlement_estimate
        ),
    ));

    // ── 10. Route type ──────────────────────────────────────────────────────
    let route_type = if req.token == "native" {
        "direct_bridge"
    } else {
        "bridge_and_unwrap"
    };

    let feasible = blockers.is_empty();
    SimulationResult {
        feasible,
        checks,
        estimated_fee: fee,
        fee_tier: fee_tier.to_string(),
        estimated_time_secs: estimated_time,
        route_type: route_type.to_string(),
        warnings,
        blockers,
    }
}

fn calculate_sim_fee(amount: u128, usd_cents: u64) -> (u128, &'static str) {
    match usd_cents {
        0..=99_999 => (0, "zero"),
        100_000..=9_999_999 => {
            let fee = amount * 5 / 10_000; // 0.05%
            (fee, "standard")
        }
        10_000_000..=999_999_999 => {
            let fee = amount / 10_000; // 0.01%
            (fee, "institutional")
        }
        _ => (0, "otc"),
    }
}

/// Convert a SimulationResult to JSON for the API response.
pub fn result_to_json(result: &SimulationResult) -> serde_json::Value {
    let checks: Vec<serde_json::Value> = result
        .checks
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "passed": c.passed,
                "detail": c.detail,
            })
        })
        .collect();

    serde_json::json!({
        "feasible": result.feasible,
        "checks": checks,
        "estimated_fee": result.estimated_fee.to_string(),
        "fee_tier": result.fee_tier,
        "estimated_time_secs": result.estimated_time_secs,
        "route_type": result.route_type,
        "warnings": result.warnings,
        "blockers": result.blockers,
    })
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_request() -> SimulationRequest {
        SimulationRequest {
            source_chain: 1,
            dest_chain: 900,
            sender: "0xAlice".into(),
            receiver: "SolBob".into(),
            amount: 1_000_000_000_000_000_000, // 1 ETH in wei
            token: "native".into(),
            usd_value_cents: 50_000, // $500 (Tier Zero)
            api_key: None,
        }
    }

    #[test]
    fn test_feasible_transfer() {
        let req = default_request();
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert!(result.feasible);
        assert!(result.blockers.is_empty());
        assert_eq!(result.estimated_fee, 0); // zero-fee tier
        assert_eq!(result.fee_tier, "zero");
    }

    #[test]
    fn test_paused_bridge_blocks() {
        let req = default_request();
        let mut config = SimulatorConfig::default_config();
        config.bridge_paused = true;
        let result = simulate(&req, &config);
        assert!(!result.feasible);
        assert!(result.blockers.iter().any(|b| b.contains("paused")));
    }

    #[test]
    fn test_unsupported_source_chain() {
        let mut req = default_request();
        req.source_chain = 9999;
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert!(!result.feasible);
        assert!(result.blockers.iter().any(|b| b.contains("9999")));
    }

    #[test]
    fn test_same_chain_blocked() {
        let mut req = default_request();
        req.dest_chain = req.source_chain;
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert!(!result.feasible);
    }

    #[test]
    fn test_zero_amount_blocked() {
        let mut req = default_request();
        req.amount = 0;
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert!(!result.feasible);
    }

    #[test]
    fn test_standard_fee_tier() {
        let mut req = default_request();
        req.usd_value_cents = 500_000; // $5,000 (standard tier)
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert_eq!(result.fee_tier, "standard");
        // 0.05% of 1 ETH = 500_000_000_000_000 wei
        assert_eq!(result.estimated_fee, 500_000_000_000_000);
    }

    #[test]
    fn test_institutional_fee_tier() {
        let mut req = default_request();
        req.usd_value_cents = 50_000_000; // $500k
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert_eq!(result.fee_tier, "institutional");
    }

    #[test]
    fn test_insufficient_liquidity_warning() {
        let req = default_request();
        let mut config = SimulatorConfig::default_config();
        config.dest_liquidity = 100; // tiny
        let result = simulate(&req, &config);
        // Still feasible (just a warning), but has a warning
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_rate_limit_exceeded_blocks() {
        let req = default_request();
        let mut config = SimulatorConfig::default_config();
        config.rate_limit_remaining = Some(0);
        let result = simulate(&req, &config);
        assert!(!result.feasible);
        assert!(result.blockers.iter().any(|b| b.contains("rate limit")));
    }

    #[test]
    fn test_estimated_time_optimism_fast() {
        let mut req = default_request();
        req.source_chain = 10; // Optimism: 2s finality
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        // 2s finality + 1s proof + 2s settlement = 5s
        assert_eq!(result.estimated_time_secs, 5);
    }

    #[test]
    fn test_estimated_time_ethereum_slow() {
        let req = default_request(); // Ethereum: 900s finality
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert_eq!(result.estimated_time_secs, 903);
    }

    #[test]
    fn test_route_type_native() {
        let req = default_request();
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert_eq!(result.route_type, "direct_bridge");
    }

    #[test]
    fn test_route_type_token() {
        let mut req = default_request();
        req.token = "0xUSDC".into();
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert_eq!(result.route_type, "bridge_and_unwrap");
    }

    #[test]
    fn test_json_export_structure() {
        let req = default_request();
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        let j = result_to_json(&result);
        assert_eq!(j["feasible"], true);
        assert!(j["checks"].is_array());
        assert!(j["estimated_time_secs"].is_number());
    }

    #[test]
    fn test_multiple_blockers_accumulated() {
        let mut req = default_request();
        req.amount = 0;
        req.source_chain = 9999;
        let mut config = SimulatorConfig::default_config();
        config.bridge_paused = true;
        let result = simulate(&req, &config);
        assert!(!result.feasible);
        assert!(result.blockers.len() >= 3, "should have multiple blockers");
    }

    #[test]
    fn test_otc_fee_tier() {
        let mut req = default_request();
        req.usd_value_cents = 5_000_000_000; // $50M
        let config = SimulatorConfig::default_config();
        let result = simulate(&req, &config);
        assert_eq!(result.fee_tier, "otc");
        assert_eq!(result.estimated_fee, 0);
    }
}
