pub mod circuit;
pub mod network;
pub mod relayer;

/// Core errors for protocol execution and network failure
#[derive(Debug)]
pub enum InterlinkError {
    ProofGenerationFailed,
    NetworkError(String),
    VerificationFailed,
}

pub type Result<T> = std::result::Result<T, InterlinkError>;

/// Trait for defining structured cross-chain messages
pub trait Message {
    fn payload(&self) -> &[u8];
    fn source_chain(&self) -> u64;
    fn dest_chain(&self) -> u64;
}
