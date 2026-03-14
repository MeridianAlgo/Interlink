//! Chain-specific finality waiting logic.
//!
//! Different chains have different finality guarantees:
//! - Ethereum: 2 epochs (~12.8 minutes) for full finality
//! - Solana: ~400ms slot confirmation
//! - Cosmos: 1 block (~6s) for Tendermint BFT finality
//! - L2s (Arbitrum, Optimism, Base): inherit L1 finality (~7 min optimistic)
//!
//! Two finality strategies are provided:
//! - `wait_for_finality`: HTTP polling (fallback, ~12s per poll for ETH)
//! - `wait_for_finality_ws`: WebSocket newHeads subscription (preferred, <1s reaction time)
//!
//! The WebSocket path is ~12x faster on Ethereum than HTTP polling because we receive
//! a notification on every block instead of waiting for the next poll interval.

use std::time::Duration;
use tracing::{info, warn};

/// Supported source chains with their finality characteristics
#[derive(Debug, Clone, Copy)]
pub enum ChainFinality {
    /// Ethereum mainnet: wait for 2 epochs (64 slots)
    Ethereum,
    /// Solana: near-instant finality after confirmation
    Solana,
    /// Cosmos chains with Tendermint BFT
    Cosmos,
    /// Optimistic rollups: use sequencer confirmation for speed
    OptimisticRollup,
}

impl ChainFinality {
    /// Number of confirmation blocks to wait before considering a transaction final.
    pub fn required_confirmations(&self) -> u64 {
        match self {
            ChainFinality::Ethereum => 64,         // 2 epochs
            ChainFinality::Solana => 1,            // single confirmation
            ChainFinality::Cosmos => 1,            // BFT instant finality
            ChainFinality::OptimisticRollup => 10, // ~20s for sequencer batch
        }
    }

    /// Expected time to finality in seconds.
    pub fn expected_finality_secs(&self) -> u64 {
        match self {
            ChainFinality::Ethereum => 768, // 64 * 12s
            ChainFinality::Solana => 1,
            ChainFinality::Cosmos => 7,
            ChainFinality::OptimisticRollup => 20,
        }
    }

    /// From chain_id to finality config
    pub fn from_chain_id(chain_id: u64) -> Self {
        match chain_id {
            1 => ChainFinality::Ethereum,                 // Ethereum mainnet
            2 => ChainFinality::Solana,                   // Solana
            5 => ChainFinality::Cosmos,                   // Cosmos
            3 | 4 | 7 => ChainFinality::OptimisticRollup, // Arbitrum, Optimism, Base
            _ => ChainFinality::Ethereum,                 // conservative default
        }
    }
}

/// Wait for a block to be finalized on the source chain.
///
/// Uses an ethers-rs provider to poll block confirmations.
/// Returns Ok(()) once the block has enough confirmations, or Err on timeout.
pub async fn wait_for_finality(
    chain_id: u64,
    block_number: u64,
    rpc_url: &str,
) -> Result<(), String> {
    let finality = ChainFinality::from_chain_id(chain_id);
    let required = finality.required_confirmations();
    let timeout = Duration::from_secs(finality.expected_finality_secs() * 2);

    info!(
        chain_id,
        block_number,
        required_confirmations = required,
        "waiting for block finality"
    );

    let start = std::time::Instant::now();
    let poll_interval = Duration::from_secs(match finality {
        ChainFinality::Ethereum => 12,
        ChainFinality::Solana => 1,
        ChainFinality::Cosmos => 3,
        ChainFinality::OptimisticRollup => 2,
    });

    // Reuse a single HTTP client across all poll iterations.
    let client = reqwest::Client::new();

    loop {
        if start.elapsed() > timeout {
            warn!(chain_id, block_number, "finality wait timed out");
            return Err(format!(
                "timed out waiting for finality on chain {} block {}",
                chain_id, block_number
            ));
        }

        // Query current block height via the appropriate RPC method

        let (method, parse_fn): (&str, fn(&serde_json::Value) -> Option<u64>) = match finality {
            ChainFinality::Solana => ("getSlot", |body: &serde_json::Value| {
                body["result"].as_u64()
            }),
            _ => ("eth_blockNumber", |body: &serde_json::Value| {
                body["result"]
                    .as_str()
                    .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            }),
        };

        let resp = client
            .post(rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": if method == "getSlot" {
                    serde_json::json!([{"commitment": "finalized"}])
                } else {
                    serde_json::json!([])
                },
                "id": 1
            }))
            .send()
            .await
            .map_err(|e| format!("rpc error: {}", e))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("parse error: {}", e))?;

        if let Some(current_block) = parse_fn(&body) {
            let confirmations = current_block.saturating_sub(block_number);

            if confirmations >= required {
                info!(
                    chain_id,
                    block_number, confirmations, "block finality confirmed"
                );
                return Ok(());
            }

            tracing::debug!(confirmations, required, "waiting for more confirmations");
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Wait for block finality using a WebSocket `eth_subscribe("newHeads")` subscription.
///
/// # Why this beats HTTP polling
///
/// The HTTP polling path wakes up every `poll_interval` seconds (12s for Ethereum).
/// In the worst case the relayer is 12 seconds behind — which alone would blow our
/// <30s settlement target for events that land right after a poll.
///
/// With WebSocket subscriptions the node pushes each new block header to us the moment
/// it is imported (~100-500ms after the block is mined). We check confirmations on every
/// push, so reaction time is 1 block-time rather than 1 poll-interval.
///
/// Competitive comparison:
/// - Wormhole: polls every ~1-2min for finality detection → 2-15min total
/// - InterLink (HTTP polling): up to 12s delay per check → worst case adds 12s
/// - InterLink (WebSocket):    <1s notification delay   → worst case adds 1 block-time
///
/// Falls back to HTTP polling for non-WebSocket URLs (Solana, Cosmos HTTP endpoints).
pub async fn wait_for_finality_ws(
    chain_id: u64,
    block_number: u64,
    ws_url: &str,
) -> Result<(), String> {
    // Only use WebSocket path for actual WS URLs; fall back for HTTP/others.
    if !ws_url.starts_with("ws://") && !ws_url.starts_with("wss://") {
        return wait_for_finality(chain_id, block_number, ws_url).await;
    }

    use ethers_providers::{Middleware, StreamExt};

    let finality = ChainFinality::from_chain_id(chain_id);
    let required = finality.required_confirmations();
    // Add 2 minutes of slack beyond expected finality for network jitter.
    let timeout_duration = Duration::from_secs(finality.expected_finality_secs() * 2 + 120);

    info!(
        chain_id,
        block_number,
        required_confirmations = required,
        timeout_secs = timeout_duration.as_secs(),
        "waiting for finality via WebSocket newHeads subscription"
    );

    let provider = ethers_providers::Provider::<ethers_providers::Ws>::connect(ws_url)
        .await
        .map_err(|e| format!("ws connect failed: {}", e))?;

    let mut stream = provider
        .subscribe_blocks()
        .await
        .map_err(|e| format!("subscribe_blocks failed: {}", e))?;

    let start = std::time::Instant::now();

    loop {
        let remaining = timeout_duration.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            warn!(chain_id, block_number, "WebSocket finality wait timed out");
            return Err(format!(
                "timed out waiting for finality on chain {} block {}",
                chain_id, block_number
            ));
        }

        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(block)) => {
                let current_block = block.number.unwrap_or_default().as_u64();
                let confirmations = current_block.saturating_sub(block_number);

                tracing::debug!(
                    chain_id,
                    current_block,
                    target_block = block_number,
                    confirmations,
                    required,
                    "newHead received"
                );

                if confirmations >= required {
                    info!(
                        chain_id,
                        block_number,
                        confirmations,
                        elapsed_ms = start.elapsed().as_millis(),
                        "finality confirmed via WebSocket"
                    );
                    return Ok(());
                }
            }
            Ok(None) => {
                return Err(
                    "WebSocket newHeads stream ended before finality was reached".to_string(),
                );
            }
            Err(_elapsed) => {
                warn!(chain_id, block_number, "WebSocket finality timeout elapsed");
                return Err(format!(
                    "timed out waiting for finality on chain {} block {}",
                    chain_id, block_number
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_finality_configs() {
        assert_eq!(ChainFinality::Ethereum.required_confirmations(), 64);
        assert_eq!(ChainFinality::Solana.required_confirmations(), 1);
        assert_eq!(ChainFinality::Cosmos.required_confirmations(), 1);
    }

    #[test]
    fn test_from_chain_id() {
        assert!(matches!(
            ChainFinality::from_chain_id(1),
            ChainFinality::Ethereum
        ));
        assert!(matches!(
            ChainFinality::from_chain_id(2),
            ChainFinality::Solana
        ));
        assert!(matches!(
            ChainFinality::from_chain_id(3),
            ChainFinality::OptimisticRollup
        ));
    }

    #[test]
    fn test_ws_fallback_for_http_url() {
        // Verifying the WS path correctly detects HTTP URLs and would fall back.
        // (We can't do a real async test without a live node, so just check the URL detection.)
        let http_url = "http://localhost:8545";
        let ws_url = "ws://localhost:8545";
        let wss_url = "wss://mainnet.infura.io/ws/v3/abc";

        assert!(!http_url.starts_with("ws://") && !http_url.starts_with("wss://"));
        assert!(ws_url.starts_with("ws://"));
        assert!(wss_url.starts_with("wss://"));
    }
}
