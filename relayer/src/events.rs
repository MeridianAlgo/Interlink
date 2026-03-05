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

/// Unified event enum for all cross-chain events the relayer observes.
#[derive(Debug, Clone)]
pub enum GatewayEvent {
    Deposit(DepositEvent),
    Swap(SwapInitiatedEvent),
    NFTLock(NFTLockedEvent),
}

impl GatewayEvent {
    pub fn sequence(&self) -> u64 {
        match self {
            GatewayEvent::Deposit(e) => e.sequence,
            GatewayEvent::Swap(e) => e.sequence,
            GatewayEvent::NFTLock(e) => e.sequence,
        }
    }

    pub fn block_number(&self) -> u64 {
        match self {
            GatewayEvent::Deposit(e) => e.block_number,
            GatewayEvent::Swap(e) => e.block_number,
            GatewayEvent::NFTLock(e) => e.block_number,
        }
    }

    pub fn payload_hash(&self) -> [u8; 32] {
        match self {
            GatewayEvent::Deposit(e) => e.payload_hash,
            GatewayEvent::Swap(e) => e.payload_hash,
            GatewayEvent::NFTLock(e) => e.nft_hash,
        }
    }
}
