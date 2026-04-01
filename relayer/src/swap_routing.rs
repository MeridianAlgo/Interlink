/// DEX swap routing aggregator for InterLink (Phase 8)
///
/// Multi-DEX aggregation for bridge+swap transfers. Finds best rate
/// across available DEX sources, handles fallback routing, and tracks
/// slippage for optimization.
///
/// Supported DEX sources:
///   EVM:    Uniswap V3, 1inch, 0x, SushiSwap, Curve
///   Solana: Jupiter, Raydium, Orca
///
/// Flow:
///   1. User requests: "bridge 1 ETH from Ethereum, receive USDC on Solana"
///   2. Router queries all available DEX sources on destination chain
///   3. Best quote selected (lowest slippage, highest output)
///   4. Fallback to next-best if primary DEX fails
///   5. Execute swap atomically with bridge settlement
///
/// Comparison:
///   LiFi:    aggregates 10+ bridges + DEXes, mature routing
///   Socket:  similar aggregation, smaller coverage
///   1inch:   DEX-only, no bridge
///   InterLink: bridge-native DEX routing with ZK-verified settlement

use std::collections::HashMap;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Maximum acceptable slippage in basis points (3%).
pub const MAX_SLIPPAGE_BPS: u32 = 300;
/// Default slippage tolerance in basis points (0.5%).
pub const DEFAULT_SLIPPAGE_BPS: u32 = 50;
/// Quote expiry time in seconds.
pub const QUOTE_EXPIRY_SECS: u64 = 30;
/// Maximum number of DEX sources to query per route.
pub const MAX_SOURCES_PER_QUERY: usize = 5;
/// Minimum output threshold: if best quote is <95% of expected, warn.
pub const MIN_OUTPUT_RATIO_BPS: u32 = 9_500;

// ─── Types ──────────────────────────────────────────────────────────────────

/// Supported DEX protocols.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DexSource {
    UniswapV3,
    OneInch,
    ZeroX,
    SushiSwap,
    Curve,
    Jupiter,
    Raydium,
    Orca,
}

impl DexSource {
    pub fn as_str(&self) -> &str {
        match self {
            DexSource::UniswapV3 => "uniswap_v3",
            DexSource::OneInch => "1inch",
            DexSource::ZeroX => "0x",
            DexSource::SushiSwap => "sushiswap",
            DexSource::Curve => "curve",
            DexSource::Jupiter => "jupiter",
            DexSource::Raydium => "raydium",
            DexSource::Orca => "orca",
        }
    }

    /// DEX sources available on a given chain.
    pub fn for_chain(chain_id: u32) -> Vec<DexSource> {
        match chain_id {
            1 | 10 | 42161 | 8453 => vec![
                DexSource::UniswapV3,
                DexSource::OneInch,
                DexSource::ZeroX,
                DexSource::SushiSwap,
            ],
            137 => vec![
                DexSource::UniswapV3,
                DexSource::OneInch,
                DexSource::SushiSwap,
                DexSource::Curve,
            ],
            900 => vec![
                DexSource::Jupiter,
                DexSource::Raydium,
                DexSource::Orca,
            ],
            _ => vec![],
        }
    }
}

/// A swap quote from a DEX source.
#[derive(Debug, Clone)]
pub struct SwapQuote {
    /// DEX that produced this quote.
    pub source: DexSource,
    /// Input token address.
    pub token_in: String,
    /// Output token address.
    pub token_out: String,
    /// Input amount.
    pub amount_in: u128,
    /// Quoted output amount.
    pub amount_out: u128,
    /// Estimated slippage in basis points.
    pub slippage_bps: u32,
    /// Gas cost estimate (in native token units).
    pub gas_estimate: u64,
    /// Quote timestamp.
    pub quoted_at: u64,
    /// Route path (e.g., ["WETH", "USDC"] or ["WETH", "WBTC", "USDC"]).
    pub path: Vec<String>,
}

/// A swap route request.
#[derive(Debug, Clone)]
pub struct SwapRequest {
    /// Chain ID where the swap occurs.
    pub chain_id: u32,
    /// Input token address.
    pub token_in: String,
    /// Output token address.
    pub token_out: String,
    /// Input amount.
    pub amount_in: u128,
    /// Maximum acceptable slippage (bps).
    pub max_slippage_bps: u32,
    /// Preferred DEX sources (empty = all available).
    pub preferred_sources: Vec<DexSource>,
}

/// Result of swap routing.
#[derive(Debug, Clone)]
pub struct SwapRoute {
    /// Best quote selected.
    pub best_quote: SwapQuote,
    /// All quotes received (sorted by output descending).
    pub all_quotes: Vec<SwapQuote>,
    /// Fallback quote (second-best).
    pub fallback: Option<SwapQuote>,
    /// Whether the best output meets minimum threshold.
    pub output_acceptable: bool,
    /// Warnings (e.g., high slippage, limited liquidity).
    pub warnings: Vec<String>,
}

/// Execution result of a swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapExecResult {
    /// Swap succeeded.
    Success {
        amount_out: u128,
        source_used: String,
        actual_slippage_bps: u32,
    },
    /// Primary DEX failed, used fallback.
    FallbackUsed {
        amount_out: u128,
        primary_source: String,
        fallback_source: String,
        primary_error: String,
    },
    /// All DEX sources failed.
    Failed {
        errors: Vec<(String, String)>,
    },
}

// ─── Swap Router ────────────────────────────────────────────────────────────

pub struct SwapRouter {
    /// Historical swap execution data for optimization.
    execution_history: Vec<SwapExecution>,
    /// Per-source reliability tracking (successes / attempts).
    source_reliability: HashMap<DexSource, (u64, u64)>,
}

/// A recorded swap execution for analytics.
#[derive(Debug, Clone)]
struct SwapExecution {
    source: DexSource,
    chain_id: u32,
    quoted_output: u128,
    actual_output: u128,
    slippage_bps: u32,
    timestamp: u64,
    success: bool,
}

impl SwapRouter {
    pub fn new() -> Self {
        SwapRouter {
            execution_history: Vec::new(),
            source_reliability: HashMap::new(),
        }
    }

    /// Find the best swap route for a request.
    /// `quotes` is a pre-fetched list of quotes from DEX sources.
    pub fn find_best_route(
        &self,
        request: &SwapRequest,
        quotes: Vec<SwapQuote>,
    ) -> Result<SwapRoute, SwapRoutingError> {
        if quotes.is_empty() {
            return Err(SwapRoutingError::NoQuotesAvailable);
        }

        // Filter by slippage tolerance
        let mut valid_quotes: Vec<SwapQuote> = quotes
            .into_iter()
            .filter(|q| q.slippage_bps <= request.max_slippage_bps)
            .collect();

        if valid_quotes.is_empty() {
            return Err(SwapRoutingError::AllQuotesExceedSlippage);
        }

        // Sort by output amount descending (best first)
        valid_quotes.sort_by(|a, b| b.amount_out.cmp(&a.amount_out));

        let best = valid_quotes[0].clone();
        let fallback = valid_quotes.get(1).cloned();

        // Check minimum output ratio
        let expected_output = request.amount_in; // 1:1 as baseline
        let output_ratio_bps = if expected_output > 0 {
            (best.amount_out as u128 * 10_000 / expected_output) as u32
        } else {
            0
        };

        let mut warnings = Vec::new();
        let output_acceptable = output_ratio_bps >= MIN_OUTPUT_RATIO_BPS || expected_output == 0;
        if !output_acceptable {
            warnings.push(format!(
                "Best output is {}bps of input — consider smaller trade",
                output_ratio_bps
            ));
        }

        if best.slippage_bps > 100 {
            warnings.push(format!(
                "Slippage {}bps on {} — above 1%",
                best.slippage_bps,
                best.source.as_str()
            ));
        }

        Ok(SwapRoute {
            best_quote: best,
            all_quotes: valid_quotes,
            fallback,
            output_acceptable,
            warnings,
        })
    }

    /// Record a swap execution for reliability tracking.
    pub fn record_execution(
        &mut self,
        source: DexSource,
        chain_id: u32,
        quoted_output: u128,
        actual_output: u128,
        timestamp: u64,
        success: bool,
    ) {
        let slippage_bps = if quoted_output > 0 && actual_output < quoted_output {
            ((quoted_output - actual_output) * 10_000 / quoted_output) as u32
        } else {
            0
        };

        self.execution_history.push(SwapExecution {
            source: source.clone(),
            chain_id,
            quoted_output,
            actual_output,
            slippage_bps,
            timestamp,
            success,
        });

        let entry = self.source_reliability.entry(source).or_insert((0, 0));
        entry.1 += 1; // attempts
        if success {
            entry.0 += 1; // successes
        }
    }

    /// Get reliability percentage for a DEX source.
    pub fn source_reliability_pct(&self, source: &DexSource) -> f64 {
        self.source_reliability
            .get(source)
            .map(|(s, a)| if *a > 0 { *s as f64 / *a as f64 * 100.0 } else { 100.0 })
            .unwrap_or(100.0) // unknown = assume reliable
    }

    /// Average actual slippage for a DEX source (bps).
    pub fn average_slippage_bps(&self, source: &DexSource) -> u32 {
        let relevant: Vec<&SwapExecution> = self
            .execution_history
            .iter()
            .filter(|e| e.source == *source && e.success)
            .collect();
        if relevant.is_empty() {
            return 0;
        }
        let total: u64 = relevant.iter().map(|e| e.slippage_bps as u64).sum();
        (total / relevant.len() as u64) as u32
    }

    /// Get supported DEX sources for a chain, sorted by reliability.
    pub fn ranked_sources(&self, chain_id: u32) -> Vec<(DexSource, f64)> {
        let mut sources: Vec<(DexSource, f64)> = DexSource::for_chain(chain_id)
            .into_iter()
            .map(|s| {
                let rel = self.source_reliability_pct(&s);
                (s, rel)
            })
            .collect();
        sources.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sources
    }

    /// Stats as JSON.
    pub fn stats_json(&self) -> serde_json::Value {
        let source_stats: Vec<serde_json::Value> = self
            .source_reliability
            .iter()
            .map(|(source, (succ, total))| {
                serde_json::json!({
                    "source": source.as_str(),
                    "successes": succ,
                    "attempts": total,
                    "reliability_pct": if *total > 0 { *succ as f64 / *total as f64 * 100.0 } else { 100.0 },
                    "avg_slippage_bps": self.average_slippage_bps(source),
                })
            })
            .collect();

        serde_json::json!({
            "total_executions": self.execution_history.len(),
            "sources": source_stats,
        })
    }
}

impl Default for SwapRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum SwapRoutingError {
    NoQuotesAvailable,
    AllQuotesExceedSlippage,
    UnsupportedChain(u32),
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_quote(source: DexSource, amount_out: u128, slippage: u32) -> SwapQuote {
        SwapQuote {
            source,
            token_in: "WETH".into(),
            token_out: "USDC".into(),
            amount_in: 1_000_000_000_000_000_000, // 1 ETH
            amount_out,
            slippage_bps: slippage,
            gas_estimate: 150_000,
            quoted_at: 1000,
            path: vec!["WETH".into(), "USDC".into()],
        }
    }

    fn sample_request() -> SwapRequest {
        SwapRequest {
            chain_id: 1,
            token_in: "WETH".into(),
            token_out: "USDC".into(),
            amount_in: 1_000_000_000_000_000_000,
            max_slippage_bps: DEFAULT_SLIPPAGE_BPS,
            preferred_sources: vec![],
        }
    }

    #[test]
    fn test_best_route_selects_highest_output() {
        let router = SwapRouter::new();
        let quotes = vec![
            mock_quote(DexSource::UniswapV3, 2_900_000_000, 30),
            mock_quote(DexSource::OneInch, 2_950_000_000, 20),
            mock_quote(DexSource::SushiSwap, 2_880_000_000, 40),
        ];
        let route = router.find_best_route(&sample_request(), quotes).unwrap();
        assert_eq!(route.best_quote.source, DexSource::OneInch);
        assert_eq!(route.best_quote.amount_out, 2_950_000_000);
        assert!(route.fallback.is_some());
        assert_eq!(route.fallback.unwrap().source, DexSource::UniswapV3);
    }

    #[test]
    fn test_slippage_filter() {
        let router = SwapRouter::new();
        let quotes = vec![
            mock_quote(DexSource::UniswapV3, 2_900_000_000, 200), // 2% > 0.5% max
            mock_quote(DexSource::OneInch, 2_800_000_000, 30),     // OK
        ];
        let route = router.find_best_route(&sample_request(), quotes).unwrap();
        assert_eq!(route.all_quotes.len(), 1);
        assert_eq!(route.best_quote.source, DexSource::OneInch);
    }

    #[test]
    fn test_all_quotes_exceed_slippage() {
        let router = SwapRouter::new();
        let quotes = vec![
            mock_quote(DexSource::UniswapV3, 2_900_000_000, 200),
            mock_quote(DexSource::OneInch, 2_800_000_000, 300),
        ];
        let result = router.find_best_route(&sample_request(), quotes);
        assert_eq!(result.unwrap_err(), SwapRoutingError::AllQuotesExceedSlippage);
    }

    #[test]
    fn test_no_quotes_available() {
        let router = SwapRouter::new();
        let result = router.find_best_route(&sample_request(), vec![]);
        assert_eq!(result.unwrap_err(), SwapRoutingError::NoQuotesAvailable);
    }

    #[test]
    fn test_high_slippage_warning() {
        let router = SwapRouter::new();
        let mut req = sample_request();
        req.max_slippage_bps = 300; // allow up to 3%
        let quotes = vec![mock_quote(DexSource::UniswapV3, 2_900_000_000, 150)];
        let route = router.find_best_route(&req, quotes).unwrap();
        assert!(route.warnings.iter().any(|w| w.contains("Slippage")));
    }

    #[test]
    fn test_dex_sources_for_ethereum() {
        let sources = DexSource::for_chain(1);
        assert!(sources.contains(&DexSource::UniswapV3));
        assert!(sources.contains(&DexSource::OneInch));
        assert!(!sources.contains(&DexSource::Jupiter));
    }

    #[test]
    fn test_dex_sources_for_solana() {
        let sources = DexSource::for_chain(900);
        assert!(sources.contains(&DexSource::Jupiter));
        assert!(sources.contains(&DexSource::Raydium));
        assert!(!sources.contains(&DexSource::UniswapV3));
    }

    #[test]
    fn test_dex_sources_unknown_chain() {
        let sources = DexSource::for_chain(9999);
        assert!(sources.is_empty());
    }

    #[test]
    fn test_record_execution_reliability() {
        let mut router = SwapRouter::new();
        router.record_execution(DexSource::UniswapV3, 1, 100, 99, 1000, true);
        router.record_execution(DexSource::UniswapV3, 1, 100, 98, 1001, true);
        router.record_execution(DexSource::UniswapV3, 1, 100, 0, 1002, false);
        assert!((router.source_reliability_pct(&DexSource::UniswapV3) - 66.66).abs() < 1.0);
    }

    #[test]
    fn test_average_slippage() {
        let mut router = SwapRouter::new();
        router.record_execution(DexSource::Jupiter, 900, 1000, 990, 100, true); // 100 bps
        router.record_execution(DexSource::Jupiter, 900, 1000, 980, 101, true); // 200 bps
        let avg = router.average_slippage_bps(&DexSource::Jupiter);
        assert_eq!(avg, 150); // (100 + 200) / 2
    }

    #[test]
    fn test_unknown_source_reliability() {
        let router = SwapRouter::new();
        assert_eq!(router.source_reliability_pct(&DexSource::Curve), 100.0);
    }

    #[test]
    fn test_ranked_sources() {
        let mut router = SwapRouter::new();
        // Uniswap: 100% reliable
        router.record_execution(DexSource::UniswapV3, 1, 100, 99, 1000, true);
        // 1inch: 50% reliable
        router.record_execution(DexSource::OneInch, 1, 100, 99, 1001, true);
        router.record_execution(DexSource::OneInch, 1, 100, 0, 1002, false);

        let ranked = router.ranked_sources(1);
        assert!(ranked.len() >= 2);
        // SushiSwap and 0x have no history → 100% default, so they may rank first
        // But Uniswap should be at 100% too
        let uni_rank = ranked.iter().find(|(s, _)| *s == DexSource::UniswapV3).unwrap();
        let inch_rank = ranked.iter().find(|(s, _)| *s == DexSource::OneInch).unwrap();
        assert!(uni_rank.1 > inch_rank.1);
    }

    #[test]
    fn test_stats_json() {
        let mut router = SwapRouter::new();
        router.record_execution(DexSource::Jupiter, 900, 1000, 990, 100, true);
        let j = router.stats_json();
        assert_eq!(j["total_executions"], 1);
        assert!(j["sources"].is_array());
    }

    #[test]
    fn test_single_quote_is_best() {
        let router = SwapRouter::new();
        let quotes = vec![mock_quote(DexSource::UniswapV3, 2_900_000_000, 10)];
        let route = router.find_best_route(&sample_request(), quotes).unwrap();
        assert_eq!(route.best_quote.source, DexSource::UniswapV3);
        assert!(route.fallback.is_none());
    }

    #[test]
    fn test_dex_source_as_str() {
        assert_eq!(DexSource::UniswapV3.as_str(), "uniswap_v3");
        assert_eq!(DexSource::OneInch.as_str(), "1inch");
        assert_eq!(DexSource::Jupiter.as_str(), "jupiter");
    }
}
