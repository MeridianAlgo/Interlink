pub mod circuit;
pub mod network;
pub mod relayer;
pub mod types;

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
                write!(f, "slippage exceeded: expected {}, got {}", expected, actual)
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
