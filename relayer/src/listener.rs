//! Event listener for EVM Gateway events (HTTP polling).
//!
//! Polls `eth_getLogs` over a block cursor for the gateway's
//! `MessagePublished` event and forwards parsed events to the pipeline.
//!
//! Polling instead of a WebSocket subscription means a dropped connection can
//! never silently skip blocks: the cursor only advances after a range has been
//! fetched and forwarded.
//!
//! Safety properties:
//! - The RPC's chain ID must match the configured source chain.
//! - Logs flagged `removed` are dropped.
//! - Malformed logs are rejected, never filled with zero defaults.
//! - Dedup is by (payload hash, block hash): a transaction re-included in a
//!   new block after a reorg is processed again. That is safe because hub
//!   attestation and EVM execution are both idempotent per payload hash.
//!
//! Reorgs after an event is forwarded are caught by
//! `finality::wait_for_finalized_inclusion`, which re-checks the receipt.

use crate::events::{CanonicalMessage, GatewayEvent, MessageEvent};
use crate::evm::{self, RpcLog};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Configuration for the event listener
#[derive(Clone, Debug)]
pub struct ListenerConfig {
    /// HTTP JSON-RPC URL of the source chain
    pub rpc_url: String,
    /// Gateway contract address on the source chain
    pub gateway_address: String,
    /// EIP-155 chain ID of the source chain (verified against the RPC)
    pub chain_id: u64,
    /// Consecutive RPC failures tolerated before the listener gives up
    pub max_consecutive_errors: u32,
    /// Block to start scanning from (None = current head)
    pub start_block: Option<u64>,
    /// Delay between polls
    pub poll_interval: Duration,
    /// Maximum blocks per `eth_getLogs` request (providers cap this)
    pub max_block_range: u64,
}

/// Event listener that watches the gateway for cross-chain events.
pub struct EventListener {
    config: ListenerConfig,
    event_tx: mpsc::Sender<GatewayEvent>,
    client: reqwest::Client,
    seen: std::collections::HashSet<([u8; 32], [u8; 32])>,
}

impl EventListener {
    pub fn new(config: ListenerConfig, event_tx: mpsc::Sender<GatewayEvent>) -> Self {
        Self {
            config,
            event_tx,
            client: reqwest::Client::new(),
            seen: std::collections::HashSet::new(),
        }
    }

    async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        evm::rpc_call(&self.client, &self.config.rpc_url, method, params).await
    }

    async fn head(&self) -> Result<u64, String> {
        let v = self.rpc("eth_blockNumber", serde_json::json!([])).await?;
        evm::quantity(v.as_str().ok_or("eth_blockNumber: not a string")?)
    }

    /// Run until the event channel closes or RPC errors exceed the limit.
    pub async fn run(&mut self) -> Result<(), String> {
        // The chain ID is part of every payload hash. Refuse to run against the
        // wrong network rather than relay mislabelled messages.
        let id = self.rpc("eth_chainId", serde_json::json!([])).await?;
        let rpc_chain_id = evm::quantity(id.as_str().ok_or("eth_chainId: not a string")?)?;
        if rpc_chain_id != self.config.chain_id {
            return Err(format!(
                "RPC chain id {} does not match configured CHAIN_ID {}",
                rpc_chain_id, self.config.chain_id
            ));
        }
        evm::hex20(&self.config.gateway_address).map_err(|e| format!("GATEWAY_ADDRESS: {e}"))?;

        let mut cursor = match self.config.start_block {
            Some(b) => b,
            None => self.head().await?,
        };
        info!(
            chain_id = rpc_chain_id,
            from_block = cursor,
            "listener started"
        );

        let mut errors = 0u32;
        loop {
            match self.poll_once(cursor).await {
                Ok(Some(next)) => {
                    cursor = next;
                    errors = 0;
                    continue; // more blocks may be waiting; poll again immediately
                }
                Ok(None) => errors = 0,
                Err(e) if e == CHANNEL_CLOSED => {
                    info!("event channel closed, stopping listener");
                    return Ok(());
                }
                Err(e) => {
                    errors += 1;
                    if errors >= self.config.max_consecutive_errors {
                        error!(errors, error = %e, "listener giving up");
                        return Err(format!(
                            "listener failed after {errors} consecutive errors: {e}"
                        ));
                    }
                    warn!(errors, error = %e, "listener poll failed; retrying");
                }
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    /// Fetch and forward one block range. Returns the next cursor, or None if
    /// there were no new blocks.
    async fn poll_once(&mut self, cursor: u64) -> Result<Option<u64>, String> {
        let head = self.head().await?;
        if cursor > head {
            return Ok(None);
        }
        let to = head.min(cursor + self.config.max_block_range.max(1) - 1);
        let logs = self
            .rpc(
                "eth_getLogs",
                serde_json::json!([{
                    "fromBlock": format!("0x{:x}", cursor),
                    "toBlock": format!("0x{:x}", to),
                    "address": self.config.gateway_address,
                    "topics": [evm::to_hex(&evm::message_published_topic())],
                }]),
            )
            .await?;
        let mut logs: Vec<RpcLog> =
            serde_json::from_value(logs).map_err(|e| format!("eth_getLogs decode: {e}"))?;
        logs.sort_by_key(|l| {
            (
                l.block_number
                    .as_deref()
                    .and_then(|b| evm::quantity(b).ok()),
                l.log_index.as_deref().and_then(|i| evm::quantity(i).ok()),
            )
        });

        for log in &logs {
            self.handle_log(log).await?;
        }
        if !logs.is_empty() {
            info!(from = cursor, to, count = logs.len(), "scanned block range");
        }
        Ok(Some(to + 1))
    }

    async fn handle_log(&mut self, log: &RpcLog) -> Result<(), String> {
        if log.removed {
            warn!(tx = ?log.transaction_hash, "dropping removed log");
            return Ok(());
        }
        let event = match parse_message_published(log, self.config.chain_id) {
            Ok(ev) => ev,
            Err(e) => {
                warn!(tx = ?log.transaction_hash, error = %e, "rejecting malformed gateway log");
                return Ok(());
            }
        };
        if !self.seen.insert((event.payload_hash, event.block_hash)) {
            return Ok(());
        }
        info!(
            nonce = event.message.nonce,
            block = event.block_number,
            "new Gateway event detected"
        );
        self.event_tx
            .send(GatewayEvent::Message(event))
            .await
            .map_err(|_| CHANNEL_CLOSED.to_string())
    }
}

const CHANNEL_CLOSED: &str = "event channel closed";

/// Read 32-byte word `i` of ABI data.
fn word(data: &[u8], i: usize) -> Result<&[u8], String> {
    data.get(i * 32..(i + 1) * 32)
        .ok_or_else(|| format!("data too short for word {i}"))
}

/// Decode a big-endian uint word that must fit in `bytes` bytes.
fn uint_word(w: &[u8], bytes: usize) -> Result<u128, String> {
    if w[..32 - bytes].iter().any(|b| *b != 0) {
        return Err(format!("value does not fit in {} bytes", bytes));
    }
    Ok(w[32 - bytes..]
        .iter()
        .fold(0u128, |acc, b| (acc << 8) | *b as u128))
}

/// Decode `MessagePublished(bytes32 indexed payloadHash, uint64 indexed nonce,
/// uint64 dstChain, bytes32 dstGateway, uint8 action, bytes32 sender,
/// bytes32 asset, uint256 amount, bytes32 recipient, bytes32 extraHash, bytes data)`.
///
/// Strict: canonical Solidity encoding only, every integer range-checked, and
/// the recomputed payload hash must equal the emitted one.
pub fn parse_message_published(log: &RpcLog, src_chain: u64) -> Result<MessageEvent, String> {
    if log.topics.len() != 3 || evm::hex32(&log.topics[0])? != evm::message_published_topic() {
        return Err("not a MessagePublished log".into());
    }
    let payload_hash = evm::hex32(&log.topics[1])?;
    let nonce = uint_word(&evm::hex32(&log.topics[2])?, 8)? as u64;

    let data = evm::from_hex(&log.data)?;
    let w32 = |i| -> Result<[u8; 32], String> { Ok(word(&data, i)?.try_into().unwrap()) };

    // Dynamic `bytes` must start right after the 9 head words.
    if uint_word(word(&data, 8)?, 8)? != 9 * 32 {
        return Err("non-canonical offset for data".into());
    }
    let len = uint_word(word(&data, 9)?, 8)? as usize;
    let padded = len.div_ceil(32) * 32;
    if data.len() != 10 * 32 + padded {
        return Err("data length does not match encoding".into());
    }
    let tail = data[10 * 32..10 * 32 + len].to_vec();

    let address = evm::hex20(&log.address)?;
    let mut src_gateway = [0u8; 32];
    src_gateway[12..].copy_from_slice(&address);

    let message = CanonicalMessage {
        src_chain,
        src_gateway,
        dst_chain: uint_word(word(&data, 0)?, 8)? as u64,
        dst_gateway: w32(1)?,
        nonce,
        action: uint_word(word(&data, 2)?, 1)? as u8,
        sender: w32(3)?,
        asset: w32(4)?,
        amount: uint_word(word(&data, 5)?, 16)?,
        recipient: w32(6)?,
        extra_hash: w32(7)?,
    };
    if message.hash() != payload_hash {
        return Err("emitted payload hash does not match event fields".into());
    }

    Ok(MessageEvent {
        block_number: evm::quantity(
            log.block_number
                .as_deref()
                .ok_or("log has no block number")?,
        )?,
        block_hash: evm::hex32(log.block_hash.as_deref().ok_or("log has no block hash")?)?,
        tx_hash: evm::hex32(
            log.transaction_hash
                .as_deref()
                .ok_or("log has no tx hash")?,
        )?,
        gateway: address,
        payload_hash,
        message,
        data: tail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{ACTION_TRANSFER, HUB_CHAIN_ID};

    fn w_u(v: u128) -> [u8; 32] {
        let mut w = [0u8; 32];
        w[16..].copy_from_slice(&v.to_be_bytes());
        w
    }

    fn sample(tamper_amount: bool, extra: &[u8]) -> (RpcLog, CanonicalMessage) {
        let mut gw = [0u8; 20];
        gw[18..].copy_from_slice(&[0xBE, 0xEF]);
        let mut src_gateway = [0u8; 32];
        src_gateway[12..].copy_from_slice(&gw);
        let msg = CanonicalMessage {
            src_chain: 1,
            src_gateway,
            dst_chain: HUB_CHAIN_ID,
            dst_gateway: [0x7B; 32],
            nonce: 3,
            action: ACTION_TRANSFER,
            sender: [0x22; 32],
            asset: [0; 32],
            amount: 5_000_000_000_000_000_000,
            recipient: [0x44; 32],
            extra_hash: evm::keccak256(extra),
        };
        let amount = if tamper_amount {
            msg.amount + 1
        } else {
            msg.amount
        };
        let mut data = Vec::new();
        for w in [
            w_u(msg.dst_chain as u128),
            msg.dst_gateway,
            w_u(msg.action as u128),
            msg.sender,
            msg.asset,
            w_u(amount),
            msg.recipient,
            msg.extra_hash,
            w_u(9 * 32),
            w_u(extra.len() as u128),
        ] {
            data.extend_from_slice(&w);
        }
        data.extend_from_slice(extra);
        data.resize(10 * 32 + extra.len().div_ceil(32) * 32, 0);

        let log = RpcLog {
            address: evm::to_hex(&gw),
            topics: vec![
                evm::to_hex(&evm::message_published_topic()),
                evm::to_hex(&msg.hash()),
                evm::to_hex(&w_u(msg.nonce as u128)),
            ],
            data: evm::to_hex(&data),
            block_number: Some("0x64".into()),
            block_hash: Some(evm::to_hex(&[9; 32])),
            transaction_hash: Some(evm::to_hex(&[8; 32])),
            log_index: Some("0x0".into()),
            removed: false,
        };
        (log, msg)
    }

    #[test]
    fn parses_message_published() {
        let (log, msg) = sample(false, b"hello world, this payload spans two words!");
        let ev = parse_message_published(&log, 1).unwrap();
        assert_eq!(ev.message, msg);
        assert_eq!(ev.payload_hash, msg.hash());
        assert_eq!(ev.block_number, 100);
        assert_eq!(ev.block_hash, [9; 32]);
        assert_eq!(ev.data, b"hello world, this payload spans two words!");
    }

    #[test]
    fn rejects_fields_that_do_not_match_hash() {
        let (log, _) = sample(true, b"");
        assert!(parse_message_published(&log, 1).is_err());
    }

    #[test]
    fn rejects_wrong_source_chain() {
        let (log, _) = sample(false, b"");
        assert!(parse_message_published(&log, 42161).is_err());
    }

    #[test]
    fn rejects_truncated_or_padded_data() {
        let (mut log, _) = sample(false, b"");
        log.data = log.data[..2 + 64 * 2].to_string();
        assert!(parse_message_published(&log, 1).is_err());

        let (mut log, _) = sample(false, b"");
        log.data.push_str("00");
        assert!(parse_message_published(&log, 1).is_err());
    }

    #[test]
    fn rejects_out_of_range_integers() {
        // action word with a non-zero high byte must not be truncated to u8
        let (mut log, _) = sample(false, b"");
        let mut data = evm::from_hex(&log.data).unwrap();
        data[2 * 32] = 1;
        log.data = evm::to_hex(&data);
        assert!(parse_message_published(&log, 1).is_err());
    }
}
