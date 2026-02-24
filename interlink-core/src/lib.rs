pub mod circuit;
pub mod network;
pub mod relayer;

/// The core error type for the InterLink protocol.
#[derive(Debug)]
pub enum InterlinkError {
    ProofGenerationFailed,
    NetworkError(String),
    VerificationFailed,
}

pub type Result<T> = std::result::Result<T, InterlinkError>;

/// A trait representing a cross-chain message.
pub trait Message {
    fn payload(&self) -> &[u8];
    fn source_chain(&self) -> u64;
    fn dest_chain(&self) -> u64;
}
