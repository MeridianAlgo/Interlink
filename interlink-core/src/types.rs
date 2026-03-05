use serde::{Deserialize, Serialize};

/// Supported blockchain networks in the InterLink ecosystem
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u16)]
pub enum Chain {
    Ethereum = 1,
    Solana = 2,
    Arbitrum = 3,
    Optimism = 4,
    Cosmos = 5,
    Sui = 6,
    Base = 7,
}

impl Chain {
    pub fn from_id(id: u16) -> Option<Self> {
        match id {
            1 => Some(Chain::Ethereum),
            2 => Some(Chain::Solana),
            3 => Some(Chain::Arbitrum),
            4 => Some(Chain::Optimism),
            5 => Some(Chain::Cosmos),
            6 => Some(Chain::Sui),
            7 => Some(Chain::Base),
            _ => None,
        }
    }

    pub fn id(&self) -> u16 {
        *self as u16
    }

    /// Returns the expected finality time in seconds for this chain.
    ///
    /// For optimistic rollups (Arbitrum, Optimism, Base), this represents the
    /// fraud proof challenge window. Funds should not be considered final until
    /// this window has elapsed.
    pub fn finality_seconds(&self) -> u64 {
        match self {
            Chain::Ethereum => 768,      // 2 epochs (~12.8 min)
            Chain::Solana => 1,          // ~400ms slot, near-instant finality
            Chain::Arbitrum => 604_800,  // 7 day fraud proof window
            Chain::Optimism => 604_800,  // 7 day fraud proof window
            Chain::Cosmos => 7,          // ~6s block time + 1 confirmation
            Chain::Sui => 3,             // ~2-3s finality
            Chain::Base => 604_800,      // 7 day fraud proof window (OP Stack)
        }
    }
}

impl std::fmt::Display for Chain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Chain::Ethereum => write!(f, "Ethereum"),
            Chain::Solana => write!(f, "Solana"),
            Chain::Arbitrum => write!(f, "Arbitrum"),
            Chain::Optimism => write!(f, "Optimism"),
            Chain::Cosmos => write!(f, "Cosmos"),
            Chain::Sui => write!(f, "Sui"),
            Chain::Base => write!(f, "Base"),
        }
    }
}

/// Types of cross-chain actions supported by InterLink
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionType {
    Transfer,
    Swap,
    ContractCall,
    NFTTransfer,
    Governance,
}

/// A structured cross-chain payload as defined in the protocol spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterLinkPayload {
    pub version: u8,
    pub action: ActionType,
    pub sender: Vec<u8>,
    pub recipient: Vec<u8>,
    pub data: Vec<u8>,
    pub deadline: u64,
}

impl InterLinkPayload {
    pub fn new(
        action: ActionType,
        sender: Vec<u8>,
        recipient: Vec<u8>,
        data: Vec<u8>,
        deadline: u64,
    ) -> Self {
        Self {
            version: 1,
            action,
            sender,
            recipient,
            data,
            deadline,
        }
    }

    /// Encode the payload to bytes for hashing
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(self.version);
        buf.push(self.action as u8);
        buf.extend_from_slice(&(self.sender.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.sender);
        buf.extend_from_slice(&(self.recipient.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.recipient);
        buf.extend_from_slice(&(self.data.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.data);
        buf.extend_from_slice(&self.deadline.to_be_bytes());
        buf
    }
}

/// Public inputs exposed by a ZK proof for on-chain verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicInputs {
    pub sequence: u64,
    pub source_chain: Chain,
    pub dest_chain: Chain,
    pub block_hash: [u8; 32],
    pub message_hash: [u8; 32],
    pub amount: u128,
}

/// A cross-chain message implementing the Message trait
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossChainMessage {
    pub sequence: u64,
    pub source_chain: Chain,
    pub dest_chain: Chain,
    pub payload: InterLinkPayload,
    pub proof: Option<Vec<u8>>,
}

impl super::Message for CrossChainMessage {
    fn payload(&self) -> &[u8] {
        &self.payload.data
    }

    fn source_chain(&self) -> u64 {
        self.source_chain.id() as u64
    }

    fn dest_chain(&self) -> u64 {
        self.dest_chain.id() as u64
    }
}

/// Status of a cross-chain message as tracked by the Hub
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageStatus {
    Pending,
    Verified,
    Executed,
    Failed,
    Refunded,
}

/// Result of a swap execution on the Hub
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapResult {
    pub amount_in: u128,
    pub amount_out: u128,
    pub fee: u128,
    pub sequence: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_roundtrip() {
        for chain in [Chain::Ethereum, Chain::Solana, Chain::Arbitrum, Chain::Cosmos] {
            assert_eq!(Chain::from_id(chain.id()), Some(chain));
        }
        assert_eq!(Chain::from_id(99), None);
    }

    #[test]
    fn test_payload_encode() {
        let payload = InterLinkPayload::new(
            ActionType::Transfer,
            vec![0xAA; 20],
            vec![0xBB; 32],
            vec![1, 2, 3, 4],
            1000,
        );
        let encoded = payload.encode();
        assert_eq!(encoded[0], 1); // version
        assert_eq!(encoded[1], 0); // Transfer action
        assert!(encoded.len() > 60);
    }

    #[test]
    fn test_cross_chain_message_trait() {
        let msg = CrossChainMessage {
            sequence: 42,
            source_chain: Chain::Ethereum,
            dest_chain: Chain::Solana,
            payload: InterLinkPayload::new(
                ActionType::Transfer,
                vec![],
                vec![],
                vec![0xDE, 0xAD],
                0,
            ),
            proof: None,
        };
        use crate::Message;
        assert_eq!(msg.source_chain(), 1);
        assert_eq!(msg.dest_chain(), 2);
        assert_eq!(msg.payload(), &[0xDE, 0xAD]);
    }
}
