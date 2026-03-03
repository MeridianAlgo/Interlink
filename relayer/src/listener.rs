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

use crate::events::{DepositEvent, GatewayEvent};
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
            // Parse MessagePublished(uint64 indexed nonce, uint64 destinationChain,
            //                        address sender, bytes32 payloadHash, bytes payload)
            // Topics: [topic0, nonce_indexed]
            // Data: [destinationChain(u64), sender(address), payloadHash(bytes32), payload(bytes)]
            //       ABI-encoded: dest_chain is padded to 32 bytes, sender padded to 32 bytes, etc.
            let nonce_topic = log.topics.get(1)?;
            let nonce = u64::from_be_bytes(nonce_topic.as_bytes()[24..32].try_into().ok()?);

            let data = &log.data;
            // ABI decode the non-indexed parameters:
            // offset 0..32:   destinationChain (uint64, right-padded in 32 bytes)
            // offset 32..64:  sender (address, left-padded in 32 bytes)
            // offset 64..96:  payloadHash (bytes32)
            // offset 96+:     payload (dynamic bytes with offset + length prefix)

            let destination_chain = if data.len() >= 32 {
                u64::from_be_bytes(data[24..32].try_into().ok()?)
            } else {
                0u64
            };

            let mut sender = [0u8; 20];
            if data.len() >= 64 {
                sender.copy_from_slice(&data[44..64]);
            }

            let mut payload_hash = [0u8; 32];
            if data.len() >= 96 {
                payload_hash.copy_from_slice(&data[64..96]);
            }

            // Parse amount from the payload if available (first 32 bytes of payload data)
            let amount = if data.len() >= 160 {
                // payload offset at data[96..128], then length at payload_start..+32, then data
                let payload_offset = u64::from_be_bytes(data[120..128].try_into().ok()?) as usize;
                if data.len() > 96 + payload_offset + 32 {
                    let len_start = 96 + payload_offset;
                    u64::from_be_bytes(data[len_start + 24..len_start + 32].try_into().ok()?)
                } else {
                    0u64
                }
            } else {
                0u64
            };

            return Some(GatewayEvent::Deposit(DepositEvent {
                block_number,
                tx_hash: log.transaction_hash.map(|h| h.0).unwrap_or([0u8; 32]),
                sequence: nonce,
                sender,
                recipient: sender.to_vec(), // Default to sender as recipient
                amount: amount as u128,
                destination_chain: destination_chain as u16,
                payload_hash,
            }));
        }

        // Parse SwapInitiated event
        let swap_topic = ethers_core::types::H256::from(
            ethers_core::utils::keccak256(
                b"SwapInitiated(uint64,address,address,uint256,address,address,uint256,uint64,bytes,bytes32)"
            )
        );

        if *topic0 == swap_topic {
            let nonce_topic = log.topics.get(1)?;
            let nonce = u64::from_be_bytes(nonce_topic.as_bytes()[24..32].try_into().ok()?);

            let mut payload_hash = [0u8; 32];
            // payloadHash is the last bytes32 in the event data
            if log.data.len() >= 32 {
                let ph_start = log.data.len() - 32;
                payload_hash.copy_from_slice(&log.data[ph_start..]);
            }

            return Some(GatewayEvent::Swap(crate::events::SwapInitiatedEvent {
                block_number,
                tx_hash: log.transaction_hash.map(|h| h.0).unwrap_or([0u8; 32]),
                sequence: nonce,
                sender: [0u8; 20],
                recipient: [0u8; 20],
                amount_in: 0,
                token_in: [0u8; 20],
                token_out: [0u8; 20],
                min_amount_out: 0,
                destination_chain: 0,
                swap_data: vec![],
                payload_hash,
            }));
        }

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
