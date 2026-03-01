pub mod circuit;
pub mod network;
pub mod relayer;

/// core errors. if something breaks, it's probably here.
#[derive(Debug)]
pub enum InterlinkError {
    ProofGenerationFailed,
    NetworkError(String),
    VerificationFailed,
}

pub type Result<T> = std::result::Result<T, InterlinkError>;

/// trait for cross-chain messages. basic building block.
pub trait Message {
    fn payload(&self) -> &[u8];
    fn source_chain(&self) -> u64;
    fn dest_chain(&self) -> u64;
}
