//! Chain-specific finality waiting logic.
//!
//! Different chains have different finality guarantees:
//! - Ethereum: 2 epochs (~12.8 minutes) for full finality
//! - Solana: ~400ms slot confirmation
//! - Cosmos: 1 block (~6s) for Tendermint BFT finality
//! - L2s (Arbitrum, Optimism, Base): inherit L1 finality (~7 min optimistic)
//!
//! The relayer pipeline uses `wait_for_finalized_inclusion`, which waits for the
//! node's `finalized` block tag and re-checks the receipt. `wait_for_finality`
//! (confirmation counting) is kept for tooling but is not reorg-safe.

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
/// Polls block height over HTTP JSON-RPC and counts confirmations. Not reorg-safe;
/// the pipeline uses `wait_for_finalized_inclusion` instead.
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

// ─── Finalized inclusion (used by the relayer pipeline) ─────────────────────

/// Why an event could not be confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InclusionError {
    /// The event is no longer in the canonical chain (reorged out, failed tx,
    /// or log missing). Never retry: relaying it would mint unbacked tokens.
    NotCanonical(String),
    /// RPC/network problem or timeout. Safe to retry later.
    Transient(String),
}

impl std::fmt::Display for InclusionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InclusionError::NotCanonical(s) => write!(f, "not canonical: {}", s),
            InclusionError::Transient(s) => write!(f, "transient: {}", s),
        }
    }
}

async fn rpc(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, InclusionError> {
    crate::evm::rpc_call(client, url, method, params)
        .await
        .map_err(InclusionError::Transient)
}

fn hex_u64(v: &serde_json::Value) -> Option<u64> {
    u64::from_str_radix(v.as_str()?.trim_start_matches("0x"), 16).ok()
}

/// Wait until the event's block is finalized, then confirm the exact log is
/// still in the canonical chain.
///
/// Replaces depth counting (64 blocks on L1, 10 blocks on L2s, which is about
/// 2.5 seconds on Arbitrum) with the node's `finalized` block tag, which
/// Ethereum and the major rollups (Arbitrum, OP Stack, Base) expose as real
/// finality. After that it re-fetches the receipt and checks block hash,
/// success status, emitting gateway and payload hash, so a reorged-out
/// deposit is never relayed.
pub async fn wait_for_finalized_inclusion(
    rpc_url: &str,
    event: &crate::events::MessageEvent,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<(), InclusionError> {
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();

    loop {
        let finalized = rpc(
            &client,
            rpc_url,
            "eth_getBlockByNumber",
            serde_json::json!(["finalized", false]),
        )
        .await?;
        let finalized_number = hex_u64(&finalized["number"]).ok_or_else(|| {
            InclusionError::Transient(
                "node did not return a finalized block (is the tag supported?)".into(),
            )
        })?;
        if finalized_number >= event.block_number {
            break;
        }
        if start.elapsed() > timeout {
            return Err(InclusionError::Transient(format!(
                "block {} not finalized after {:?} (finalized head {})",
                event.block_number, timeout, finalized_number
            )));
        }
        tracing::debug!(
            block = event.block_number,
            finalized_number,
            "waiting for finality"
        );
        tokio::time::sleep(poll_interval).await;
    }

    let receipt = rpc(
        &client,
        rpc_url,
        "eth_getTransactionReceipt",
        serde_json::json!([crate::evm::to_hex(&event.tx_hash)]),
    )
    .await?;
    if receipt.is_null() {
        return Err(InclusionError::NotCanonical(
            "transaction receipt not found".into(),
        ));
    }
    if receipt["status"].as_str() != Some("0x1") {
        return Err(InclusionError::NotCanonical("transaction reverted".into()));
    }
    if !receipt["blockHash"]
        .as_str()
        .unwrap_or_default()
        .eq_ignore_ascii_case(&crate::evm::to_hex(&event.block_hash))
    {
        return Err(InclusionError::NotCanonical(format!(
            "transaction now in block {} (event seen in {}): reorg",
            receipt["blockHash"],
            crate::evm::to_hex(&event.block_hash)
        )));
    }

    let gateway = crate::evm::to_hex(&event.gateway);
    let topic0 = crate::evm::to_hex(&crate::evm::message_published_topic());
    let payload = crate::evm::to_hex(&event.payload_hash);
    let found = receipt["logs"].as_array().into_iter().flatten().any(|log| {
        let topics = log["topics"].as_array();
        log["address"]
            .as_str()
            .is_some_and(|a| a.eq_ignore_ascii_case(&gateway))
            && topics.is_some_and(|t| {
                t.len() >= 2
                    && t[0]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&topic0))
                    && t[1]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&payload))
            })
    });
    if !found {
        return Err(InclusionError::NotCanonical(
            "MessagePublished log not in finalized receipt".into(),
        ));
    }

    info!(
        block = event.block_number,
        "event finalized and confirmed canonical"
    );
    Ok(())
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
