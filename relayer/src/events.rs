//! Event type definitions for cross-chain messages observed by the relayer.

use serde::{Deserialize, Serialize};

/// A deposit event emitted by the EVM Gateway's sendCrossChainMessage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositEvent {
    pub block_number: u64,
    pub tx_hash: [u8; 32],
    pub sequence: u64,
    pub sender: [u8; 20],
    pub recipient: Vec<u8>,
    pub amount: u128,
    pub destination_chain: u16,
    pub payload_hash: [u8; 32],
}

/// A swap event emitted by the EVM Gateway's initiateSwap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapInitiatedEvent {
    pub block_number: u64,
    pub tx_hash: [u8; 32],
    pub sequence: u64,
    pub sender: [u8; 20],
    pub recipient: [u8; 20],
    pub amount_in: u128,
    pub token_in: [u8; 20],
    pub token_out: [u8; 20],
    pub min_amount_out: u128,
    pub destination_chain: u16,
    pub swap_data: Vec<u8>,
    pub payload_hash: [u8; 32],
}

/// An NFT lock event emitted by the EVM Gateway's lockNFT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NFTLockedEvent {
    pub block_number: u64,
    pub tx_hash: [u8; 32],
    pub sequence: u64,
    pub sender: [u8; 20],
    pub nft_contract: [u8; 20],
    pub token_id: [u8; 32],
    pub destination_chain: u16,
    pub destination_recipient: [u8; 32],
    pub nft_hash: [u8; 32],
}

/// Message actions (must match InterlinkGateway and the Solana hub).
pub const ACTION_TRANSFER: u8 = 1;
pub const ACTION_SWAP: u8 = 2;
pub const ACTION_NFT: u8 = 3;
pub const ACTION_CALL: u8 = 4;

/// Chain ID of the Solana hub in the canonical message format.
pub const HUB_CHAIN_ID: u64 = 2;

/// Canonical cross-chain message. `hash()` must equal
/// `InterlinkGateway.hashMessage` and the hub's `canonical_hash`:
///
/// keccak256(abi.encode(keccak256("InterlinkMessage(v1)"), srcChain, srcGateway,
///   dstChain, dstGateway, nonce, action, sender, asset, amount, recipient, extraHash))
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalMessage {
    pub src_chain: u64,
    pub src_gateway: [u8; 32],
    pub dst_chain: u64,
    pub dst_gateway: [u8; 32],
    pub nonce: u64,
    pub action: u8,
    pub sender: [u8; 32],
    pub asset: [u8; 32],
    pub amount: u128,
    pub recipient: [u8; 32],
    pub extra_hash: [u8; 32],
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

impl CanonicalMessage {
    pub fn hash(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(12 * 32);
        buf.extend_from_slice(&crate::evm::keccak256(b"InterlinkMessage(v1)"));
        buf.extend_from_slice(&word_u64(self.src_chain));
        buf.extend_from_slice(&self.src_gateway);
        buf.extend_from_slice(&word_u64(self.dst_chain));
        buf.extend_from_slice(&self.dst_gateway);
        buf.extend_from_slice(&word_u64(self.nonce));
        buf.extend_from_slice(&word_u64(self.action as u64));
        buf.extend_from_slice(&self.sender);
        buf.extend_from_slice(&self.asset);
        buf.extend_from_slice(&word_u128(self.amount));
        buf.extend_from_slice(&self.recipient);
        buf.extend_from_slice(&self.extra_hash);
        crate::evm::keccak256(&buf)
    }
}

/// `MessagePublished` emitted by the current InterlinkGateway for every outbound action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEvent {
    pub block_number: u64,
    /// Hash of the block the log was seen in; re-checked after finality to detect reorgs.
    pub block_hash: [u8; 32],
    pub tx_hash: [u8; 32],
    /// Gateway contract that emitted the log.
    pub gateway: [u8; 20],
    /// Payload hash as emitted (indexed topic). Must equal `message.hash()`.
    pub payload_hash: [u8; 32],
    pub message: CanonicalMessage,
    /// Action-specific data (payload, swap params, NFT token id).
    pub data: Vec<u8>,
}

/// Unified event enum for all cross-chain events the relayer observes.
///
/// `Message` is what the current gateway emits. The other variants describe
/// the previous event format and are kept for the benchmark/load-test tools;
/// they cannot be submitted to the hub.
#[derive(Debug, Clone)]
pub enum GatewayEvent {
    Message(MessageEvent),
    Deposit(DepositEvent),
    Swap(SwapInitiatedEvent),
    NFTLock(NFTLockedEvent),
}

impl GatewayEvent {
    pub fn sequence(&self) -> u64 {
        match self {
            GatewayEvent::Message(e) => e.message.nonce,
            GatewayEvent::Deposit(e) => e.sequence,
            GatewayEvent::Swap(e) => e.sequence,
            GatewayEvent::NFTLock(e) => e.sequence,
        }
    }

    pub fn block_number(&self) -> u64 {
        match self {
            GatewayEvent::Message(e) => e.block_number,
            GatewayEvent::Deposit(e) => e.block_number,
            GatewayEvent::Swap(e) => e.block_number,
            GatewayEvent::NFTLock(e) => e.block_number,
        }
    }

    pub fn payload_hash(&self) -> [u8; 32] {
        match self {
            GatewayEvent::Message(e) => e.payload_hash,
            GatewayEvent::Deposit(e) => e.payload_hash,
            GatewayEvent::Swap(e) => e.payload_hash,
            GatewayEvent::NFTLock(e) => e.nft_hash,
        }
    }

    /// The canonical message, but only if its recomputed hash equals the
    /// emitted payload hash. The relayer never proves a hash it cannot
    /// reconstruct from the event fields.
    pub fn verified_message(&self) -> Option<&CanonicalMessage> {
        match self {
            GatewayEvent::Message(e) if e.message.hash() == e.payload_hash => Some(&e.message),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word_addr(last2: [u8; 2]) -> [u8; 32] {
        let mut w = [0u8; 32];
        w[30..].copy_from_slice(&last2);
        w
    }

    /// Fixed vector shared with contracts/evm/test (VECTOR_HASH), which was
    /// checked against the compiled gateway in an EVM. If this test fails the
    /// relayer would prove hashes no gateway accepts.
    #[test]
    fn canonical_hash_matches_evm_vector() {
        let msg = CanonicalMessage {
            src_chain: HUB_CHAIN_ID,
            src_gateway: [0x7B; 32],
            dst_chain: 31337,
            dst_gateway: word_addr([0xBE, 0xEF]),
            nonce: 7,
            action: ACTION_TRANSFER,
            sender: [0x22; 32],
            asset: [0; 32],
            amount: 2_000_000_000_000_000_000,
            recipient: {
                let mut w = [0u8; 32];
                w[12..].copy_from_slice(&[0xA1; 20]);
                w
            },
            extra_hash: [0; 32],
        };
        let expected: [u8; 32] = crate::evm::from_hex(
            "07b0f8b8ce5c6f82e0bb552073f1c35df9b0987c6e2c598c52ed5b59ee1c09bd",
        )
        .unwrap()
        .try_into()
        .unwrap();
        assert_eq!(msg.hash(), expected);
    }

    #[test]
    fn verified_message_rejects_mismatched_hash() {
        let message = CanonicalMessage {
            src_chain: 1,
            src_gateway: [1; 32],
            dst_chain: HUB_CHAIN_ID,
            dst_gateway: [2; 32],
            nonce: 0,
            action: ACTION_TRANSFER,
            sender: [3; 32],
            asset: [0; 32],
            amount: 10,
            recipient: [4; 32],
            extra_hash: [5; 32],
        };
        let good = MessageEvent {
            block_number: 1,
            block_hash: [0; 32],
            tx_hash: [0; 32],
            gateway: [0; 20],
            payload_hash: message.hash(),
            message: message.clone(),
            data: vec![],
        };
        assert!(GatewayEvent::Message(good.clone())
            .verified_message()
            .is_some());

        let mut bad = good;
        bad.message.amount = 10_000;
        assert!(GatewayEvent::Message(bad).verified_message().is_none());
    }
}
