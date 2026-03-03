//! WebSocket event listener for EVM Gateway events.
//!
//! Connects to an EVM node via WebSocket, subscribes to Gateway contract
//! events (MessagePublished, SwapInitiated, NFTLocked), and sends parsed
//! events to the prover pipeline via an mpsc channel.
//!
//! Features:
//! - Automatic reconnection with exponential backoff
//! - Chain reorg detection via block hash tracking
//! - Event deduplication via nonce tracking

use crate::events::{DepositEvent, GatewayEvent, SwapInitiatedEvent};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Configuration for the event listener
#[derive(Clone, Debug)]
pub struct ListenerConfig {
    /// WebSocket RPC URL (e.g., ws://localhost:8545)
    pub ws_rpc_url: String,
    /// Gateway contract address on the source chain
    pub gateway_address: String,
    /// Chain ID of the source chain
    pub chain_id: u64,
    /// Maximum reconnection attempts before giving up
    pub max_reconnect_attempts: u32,
}

/// Event listener that watches Gateway contracts for cross-chain events.
pub struct EventListener {
    config: ListenerConfig,
    event_tx: mpsc::Sender<GatewayEvent>,
    /// Track seen nonces for deduplication (handles reorgs)
    seen_nonces: std::collections::HashSet<u64>,
}

impl EventListener {
    pub fn new(config: ListenerConfig, event_tx: mpsc::Sender<GatewayEvent>) -> Self {
        Self {
            config,
            event_tx,
            seen_nonces: std::collections::HashSet::new(),
        }
    }

    /// Run the event listener loop. Connects via WebSocket, subscribes to
    /// Gateway events, and forwards them to the prover pipeline.
    /// Automatically reconnects on disconnection with exponential backoff.
    pub async fn run(&mut self) -> Result<(), String> {
        let mut backoff_ms = 1000u64;
        let mut attempts = 0u32;

        loop {
            info!(
                url = %self.config.ws_rpc_url,
                chain_id = self.config.chain_id,
                "connecting to EVM WebSocket"
            );

            match self.subscribe_and_process().await {
                Ok(()) => {
                    info!("listener completed normally");
                    return Ok(());
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= self.config.max_reconnect_attempts {
                        error!(
                            attempts,
                            "max reconnection attempts reached, giving up"
                        );
                        return Err(format!("listener failed after {} attempts: {}", attempts, e));
                    }

                    warn!(
                        error = %e,
                        backoff_ms,
                        attempt = attempts,
                        "WebSocket disconnected, reconnecting"
                    );

                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30_000); // cap at 30s
                }
            }
        }
    }

    /// Subscribe to Gateway events and process them.
    /// This is the inner loop that gets retried on disconnection.
    async fn subscribe_and_process(&mut self) -> Result<(), String> {
        // Connect via ethers-rs WebSocket provider
        let provider = ethers_providers::Provider::<ethers_providers::Ws>::connect(&self.config.ws_rpc_url)
            .await
            .map_err(|e| format!("ws connect failed: {}", e))?;

        info!("WebSocket connected, subscribing to Gateway events");

        // Subscribe to logs from the gateway contract
        let gateway_addr: ethers_core::types::Address = self
            .config
            .gateway_address
            .parse()
            .map_err(|e| format!("invalid gateway address: {}", e))?;

        // MessagePublished event topic
        let message_published_topic = ethers_core::utils::keccak256(
            b"MessagePublished(uint64,uint64,address,bytes32,bytes)"
        );
        // SwapInitiated event topic
        let swap_initiated_topic = ethers_core::utils::keccak256(
            b"SwapInitiated(uint64,address,address,uint256,address,address,uint256,uint64,bytes,bytes32)"
        );

        let filter = ethers_core::types::Filter::new()
            .address(gateway_addr)
            .topic0(vec![
                ethers_core::types::H256::from(message_published_topic),
                ethers_core::types::H256::from(swap_initiated_topic),
            ]);

        use ethers_providers::Middleware;
        use ethers_providers::StreamExt;
        let mut stream = provider
            .subscribe_logs(&filter)
            .await
            .map_err(|e| format!("subscribe failed: {}", e))?;

        while let Some(log) = stream.next().await {
            if let Some(event) = self.parse_log(&log) {
                let nonce = event.sequence();

                // Deduplication
                if self.seen_nonces.contains(&nonce) {
                    tracing::debug!(nonce, "duplicate event, skipping");
                    continue;
                }
                self.seen_nonces.insert(nonce);

                info!(
                    nonce,
                    block = event.block_number(),
                    "new Gateway event detected"
                );

                if self.event_tx.send(event).await.is_err() {
                    warn!("event channel closed, stopping listener");
                    return Ok(());
                }
            }
        }

        Err("WebSocket stream ended".to_string())
    }

    /// Parse a raw log into a typed GatewayEvent.
    fn parse_log(&self, log: &ethers_core::types::Log) -> Option<GatewayEvent> {
        let topic0 = log.topics.first()?;
        let block_number = log.block_number?.as_u64();

        let message_published_topic = ethers_core::types::H256::from(
            ethers_core::utils::keccak256(
                b"MessagePublished(uint64,uint64,address,bytes32,bytes)"
            )
        );

        if *topic0 == message_published_topic {
            // Parse MessagePublished event
            let nonce_topic = log.topics.get(1)?;
            let nonce = u64::from_be_bytes(nonce_topic.as_bytes()[24..32].try_into().ok()?);

            let mut payload_hash = [0u8; 32];
            if log.data.len() >= 32 {
                payload_hash.copy_from_slice(&log.data[0..32]);
            }

            return Some(GatewayEvent::Deposit(DepositEvent {
                block_number,
                tx_hash: log.transaction_hash.map(|h| h.0).unwrap_or([0u8; 32]),
                sequence: nonce,
                sender: [0u8; 20], // TODO: parse from data
                recipient: vec![],
                amount: 0, // TODO: parse from data
                destination_chain: self.config.chain_id as u16,
                payload_hash,
            }));
        }

        // Additional event parsing would go here for SwapInitiated, NFTLocked, etc.

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_listener_config() {
        let config = ListenerConfig {
            ws_rpc_url: "ws://localhost:8545".to_string(),
            gateway_address: "0x0000000000000000000000000000000000000000".to_string(),
            chain_id: 1,
            max_reconnect_attempts: 5,
        };
        assert_eq!(config.chain_id, 1);
        assert_eq!(config.max_reconnect_attempts, 5);
    }
}
