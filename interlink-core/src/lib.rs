//! Halo2 circuit primitives (Poseidon chip) and payload types.
//!
//! The production relayer lives in the `relayer` crate and uses Groth16.
//! An older standalone relayer that used to live here was removed: it produced
//! Halo2 proofs the on-chain Groth16 verifiers cannot check, and pulled in
//! ethers-providers (and through it the vulnerable ring 0.16).

pub mod circuit;
pub mod types;

/// keccak256, used to derive the circuit round constant.
pub fn keccak256(data: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    Keccak256::digest(data).into()
}

/// Core errors for protocol execution and network failure
#[derive(Debug)]
pub enum InterlinkError {
    ProofGenerationFailed,
    NetworkError(String),
    VerificationFailed,
    InvalidChain(u16),
    InvalidSequence(u64),
    SlippageExceeded { expected: u128, actual: u128 },
    Timeout,
}

impl std::fmt::Display for InterlinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InterlinkError::ProofGenerationFailed => write!(f, "proof generation failed"),
            InterlinkError::NetworkError(e) => write!(f, "network error: {}", e),
            InterlinkError::VerificationFailed => write!(f, "proof verification failed"),
            InterlinkError::InvalidChain(id) => write!(f, "invalid chain id: {}", id),
            InterlinkError::InvalidSequence(seq) => write!(f, "invalid sequence: {}", seq),
            InterlinkError::SlippageExceeded { expected, actual } => {
                write!(
                    f,
                    "slippage exceeded: expected {}, got {}",
                    expected, actual
                )
            }
            InterlinkError::Timeout => write!(f, "operation timed out"),
        }
    }
}

impl std::error::Error for InterlinkError {}

pub type Result<T> = std::result::Result<T, InterlinkError>;

/// Trait for defining structured cross-chain messages
pub trait Message {
    fn payload(&self) -> &[u8];
    fn source_chain(&self) -> u64;
    fn dest_chain(&self) -> u64;
}
