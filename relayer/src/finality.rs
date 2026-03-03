//! Chain-specific finality waiting logic.
//!
//! Different chains have different finality guarantees:
//! - Ethereum: 2 epochs (~12.8 minutes) for full finality
//! - Solana: ~400ms slot confirmation
//! - Cosmos: 1 block (~6s) for Tendermint BFT finality
//! - L2s (Arbitrum, Optimism, Base): inherit L1 finality (~7 min optimistic)

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
            ChainFinality::Ethereum => 64,           // 2 epochs
            ChainFinality::Solana => 1,              // single confirmation
            ChainFinality::Cosmos => 1,              // BFT instant finality
            ChainFinality::OptimisticRollup => 10,   // ~20s for sequencer batch
        }
    }

    /// Expected time to finality in seconds.
    pub fn expected_finality_secs(&self) -> u64 {
        match self {
            ChainFinality::Ethereum => 768,   // 64 * 12s
            ChainFinality::Solana => 1,
            ChainFinality::Cosmos => 7,
            ChainFinality::OptimisticRollup => 20,
        }
    }

    /// From chain_id to finality config
    pub fn from_chain_id(chain_id: u64) -> Self {
        match chain_id {
            1 => ChainFinality::Ethereum,         // Ethereum mainnet
            2 => ChainFinality::Solana,            // Solana
            5 => ChainFinality::Cosmos,            // Cosmos
            3 | 4 | 7 => ChainFinality::OptimisticRollup, // Arbitrum, Optimism, Base
            _ => ChainFinality::Ethereum,          // conservative default
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

    loop {
        if start.elapsed() > timeout {
            warn!(chain_id, block_number, "finality wait timed out");
            return Err(format!(
                "timed out waiting for finality on chain {} block {}",
                chain_id, block_number
            ));
        }

        // Query current block height via JSON-RPC
        let client = reqwest::Client::new();
        let resp = client
            .post(rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 1
            }))
            .send()
            .await
            .map_err(|e| format!("rpc error: {}", e))?;

        let body: serde_json::Value = resp.json().await.map_err(|e| format!("parse error: {}", e))?;

        if let Some(hex_str) = body["result"].as_str() {
            let current_block = u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)
                .unwrap_or(0);

            let confirmations = current_block.saturating_sub(block_number);

            if confirmations >= required {
                info!(
                    chain_id,
                    block_number,
                    confirmations,
                    "block finality confirmed"
                );
                return Ok(());
            }

            tracing::debug!(
                confirmations,
                required,
                "waiting for more confirmations"
            );
        }

        tokio::time::sleep(poll_interval).await;
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
        assert!(matches!(ChainFinality::from_chain_id(1), ChainFinality::Ethereum));
        assert!(matches!(ChainFinality::from_chain_id(2), ChainFinality::Solana));
        assert!(matches!(ChainFinality::from_chain_id(3), ChainFinality::OptimisticRollup));
    }
}
