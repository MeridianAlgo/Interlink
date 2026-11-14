//! Delivers hub → EVM messages by calling `InterlinkGateway.releaseTokens`.

use crate::events::CanonicalMessage;
use crate::evm::{self, keccak256};
use crate::evm_tx::{self, Eip1559Tx};
use k256::ecdsa::SigningKey;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

const RECEIPT_TIMEOUT_SECS: u64 = 180;
const DEFAULT_PRIORITY_FEE_WEI: u128 = 1_500_000_000;

#[derive(Clone, Debug)]
pub struct EvmSubmitterConfig {
    pub rpc_url: String,
    pub chain_id: u64,
    pub gateway: [u8; 20],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseOutcome {
    Confirmed(String),
    /// The gateway already executed this message (another relayer, or an earlier attempt).
    AlreadyExecuted,
}

#[derive(Clone)]
pub struct EvmSubmitter {
    config: EvmSubmitterConfig,
    client: reqwest::Client,
    key: Arc<SigningKey>,
    from: [u8; 20],
    /// One transaction at a time per key, so nonces never collide.
    send_lock: Arc<Mutex<()>>,
}

fn selector(sig: &str) -> [u8; 4] {
    keccak256(sig.as_bytes())[..4].try_into().unwrap()
}

fn word_u64(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

fn word_u128(v: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

/// Calldata for `releaseTokens((uint64,uint64,bytes32,bytes32,uint256,bytes32,bytes32),bytes)`.
pub fn release_calldata(m: &CanonicalMessage, proof: &[u8]) -> Vec<u8> {
    let mut d =
        selector("releaseTokens((uint64,uint64,bytes32,bytes32,uint256,bytes32,bytes32),bytes)")
            .to_vec();
    for w in [
        word_u64(m.src_chain),
        word_u64(m.nonce),
        m.sender,
        m.asset,
        word_u128(m.amount),
        m.recipient,
        m.extra_hash,
        word_u64(8 * 32), // offset of `proof`, after the 7 tuple words + this word
        word_u64(proof.len() as u64),
    ] {
        d.extend_from_slice(&w);
    }
    d.extend_from_slice(proof);
    d.resize(d.len() + (32 - proof.len() % 32) % 32, 0);
    d
}

fn quantity_u128(v: &serde_json::Value) -> Result<u128, String> {
    let s = v.as_str().ok_or("expected hex quantity")?;
    u128::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16)
        .map_err(|e| format!("bad quantity {s}: {e}"))
}

impl EvmSubmitter {
    pub fn new(config: EvmSubmitterConfig, key: SigningKey) -> Self {
        let from = evm_tx::address(&key);
        Self {
            config,
            client: reqwest::Client::new(),
            key: Arc::new(key),
            from,
            send_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn address(&self) -> [u8; 20] {
        self.from
    }

    async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        evm::rpc_call(&self.client, &self.config.rpc_url, method, params).await
    }

    async fn already_executed(&self, payload_hash: &[u8; 32]) -> Result<bool, String> {
        let mut data = selector("executedMessages(bytes32)").to_vec();
        data.extend_from_slice(payload_hash);
        let out = self
            .rpc(
                "eth_call",
                serde_json::json!([{"to": evm::to_hex(&self.config.gateway), "data": evm::to_hex(&data)}, "latest"]),
            )
            .await?;
        let bytes = evm::from_hex(out.as_str().ok_or("eth_call: not a string")?)?;
        Ok(bytes.last() == Some(&1))
    }

    /// Release tokens on the gateway for a proven hub message.
    pub async fn release(
        &self,
        m: &CanonicalMessage,
        proof: &[u8],
    ) -> Result<ReleaseOutcome, String> {
        let payload_hash = m.hash();
        if self.already_executed(&payload_hash).await? {
            return Ok(ReleaseOutcome::AlreadyExecuted);
        }
        let data = release_calldata(m, proof);
        let to = evm::to_hex(&self.config.gateway);
        let from = evm::to_hex(&self.from);

        let _guard = self.send_lock.lock().await;

        // A revert here (bad proof, unknown source, insufficient custody) is
        // surfaced before spending gas.
        let gas = self
            .rpc(
                "eth_estimateGas",
                serde_json::json!([{"from": from, "to": to, "data": evm::to_hex(&data)}]),
            )
            .await
            .map_err(|e| format!("releaseTokens would revert: {e}"))?;
        let gas_limit = (quantity_u128(&gas)? as u64).saturating_mul(12) / 10;

        let nonce = quantity_u128(
            &self
                .rpc(
                    "eth_getTransactionCount",
                    serde_json::json!([from, "pending"]),
                )
                .await?,
        )? as u64;
        let block = self
            .rpc("eth_getBlockByNumber", serde_json::json!(["latest", false]))
            .await?;
        let base_fee = quantity_u128(&block["baseFeePerGas"]).unwrap_or(0);
        let priority = match self
            .rpc("eth_maxPriorityFeePerGas", serde_json::json!([]))
            .await
        {
            Ok(v) => quantity_u128(&v).unwrap_or(DEFAULT_PRIORITY_FEE_WEI),
            Err(_) => DEFAULT_PRIORITY_FEE_WEI,
        };

        let tx = Eip1559Tx {
            chain_id: self.config.chain_id,
            nonce,
            max_priority_fee_per_gas: priority,
            max_fee_per_gas: base_fee.saturating_mul(2).saturating_add(priority),
            gas_limit,
            to: self.config.gateway,
            value: 0,
            data,
        };
        let raw = tx.sign(&self.key)?;
        let hash = self
            .rpc(
                "eth_sendRawTransaction",
                serde_json::json!([evm::to_hex(&raw)]),
            )
            .await?;
        let tx_hash = hash
            .as_str()
            .ok_or("eth_sendRawTransaction: no hash")?
            .to_string();
        info!(tx = %tx_hash, nonce = m.nonce, "releaseTokens sent");

        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(RECEIPT_TIMEOUT_SECS);
        while std::time::Instant::now() < deadline {
            let receipt = self
                .rpc("eth_getTransactionReceipt", serde_json::json!([tx_hash]))
                .await?;
            if !receipt.is_null() {
                return if receipt["status"].as_str() == Some("0x1") {
                    Ok(ReleaseOutcome::Confirmed(tx_hash))
                } else if self.already_executed(&payload_hash).await.unwrap_or(false) {
                    Ok(ReleaseOutcome::AlreadyExecuted)
                } else {
                    Err(format!("releaseTokens reverted in {tx_hash}"))
                };
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        warn!(tx = %tx_hash, "releaseTokens not mined in time");
        Err(format!(
            "transaction {tx_hash} not mined within {RECEIPT_TIMEOUT_SECS}s"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{ACTION_TRANSFER, HUB_CHAIN_ID};

    #[test]
    fn release_calldata_layout() {
        let m = CanonicalMessage {
            src_chain: HUB_CHAIN_ID,
            src_gateway: [0x7B; 32],
            dst_chain: 1,
            dst_gateway: [0; 32],
            nonce: 7,
            action: ACTION_TRANSFER,
            sender: [0x22; 32],
            asset: [0; 32],
            amount: 2,
            recipient: [0xA1; 32],
            extra_hash: [0; 32],
        };
        let d = release_calldata(&m, &[9u8; 256]);
        assert_eq!(
            &d[..4],
            &selector(
                "releaseTokens((uint64,uint64,bytes32,bytes32,uint256,bytes32,bytes32),bytes)"
            )
        );
        assert_eq!(d.len(), 4 + 9 * 32 + 256);
        assert_eq!(&d[4 + 24..4 + 32], &HUB_CHAIN_ID.to_be_bytes());
        assert_eq!(&d[4 + 7 * 32 + 24..4 + 8 * 32], &256u64.to_be_bytes());

        // Proof lengths that are not a multiple of 32 are zero-padded.
        assert_eq!(release_calldata(&m, &[1u8; 33]).len(), 4 + 9 * 32 + 64);
    }
}
