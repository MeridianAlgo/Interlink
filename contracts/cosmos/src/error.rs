use cosmwasm_std::StdError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("unauthorized: only the guardian can perform this action")]
    Unauthorized,

    #[error("contract is paused")]
    Paused,

    #[error("contract must be paused for this action")]
    NotPaused,

    #[error("message {nonce} from chain {source_chain} has already been executed")]
    AlreadyExecuted { source_chain: u64, nonce: u64 },

    #[error("invalid proof: verification failed")]
    InvalidProof,

    /// Returned for every inbound message until a real on-chain proof verifier
    /// exists. Failing closed is deliberate; see `verify_proof` in contract.rs.
    #[error("no on-chain proof verifier is configured; inbound messages are disabled")]
    VerifierUnavailable,

    #[error("insufficient funds: required {required}, available {available}")]
    InsufficientFunds { required: u128, available: u128 },

    #[error("invalid destination chain: {chain_id}")]
    InvalidDestinationChain { chain_id: u16 },

    #[error("zero amount not allowed")]
    ZeroAmount,

    #[error("send exactly one coin denomination per message")]
    MultipleDenoms,

    #[error("fee rate {fee_rate_bps} bps exceeds maximum of {max} bps")]
    InvalidFeeRate { fee_rate_bps: u16, max: u16 },

    #[error("no pending guardian to accept")]
    NoPendingGuardian,
}
