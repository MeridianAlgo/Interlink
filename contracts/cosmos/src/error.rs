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

    #[error("message with nonce {nonce} has already been executed")]
    AlreadyExecuted { nonce: u64 },

    #[error("invalid proof: verification failed")]
    InvalidProof,

    #[error("insufficient funds: required {required}, available {available}")]
    InsufficientFunds { required: u128, available: u128 },

    #[error("invalid destination chain: {chain_id}")]
    InvalidDestinationChain { chain_id: u16 },

    #[error("zero amount not allowed")]
    ZeroAmount,
}
