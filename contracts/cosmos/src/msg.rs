use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::Uint128;

#[cw_serde]
pub struct InstantiateMsg {
    /// The DAO guardian address that can pause/unpause and perform emergency actions
    pub guardian: String,
    /// The chain ID of the Solana Hub (destination for proofs)
    pub hub_chain_id: u16,
    /// Fee rate in basis points (e.g., 10 = 0.1%). Capped at `MAX_FEE_RATE_BPS`.
    pub fee_rate_bps: u16,
}

#[cw_serde]
pub enum ExecuteMsg {
    /// Send a cross-chain message by locking exactly one native coin denomination.
    /// The relayer network will observe the resulting event and generate a ZK proof.
    SendCrossChainMessage {
        recipient: Vec<u8>,
        destination_chain: u16,
        payload: Vec<u8>,
    },

    /// Execute a verified message from the Hub, releasing locked tokens.
    ///
    /// Every field that affects where funds go is hashed on-chain into the
    /// proof's public input, so a relayer cannot change recipient, denom or
    /// amount after the proof is generated.
    ExecuteVerifiedMessage {
        source_chain: u64,
        nonce: u64,
        recipient: String,
        denom: String,
        amount: Uint128,
        proof: Vec<u8>,
    },

    /// Initiate a cross-chain swap. Locks input tokens and emits a SwapInitiated event.
    InitiateSwap {
        recipient: Vec<u8>,
        destination_chain: u16,
        token_out: String,
        min_amount_out: Uint128,
        swap_data: Vec<u8>,
    },

    /// Pause the contract (guardian only)
    Pause {},

    /// Unpause the contract (guardian only)
    Unpause {},

    /// Withdraw all funds of `denom` to the guardian. Guardian only, and only
    /// while paused, so it cannot be used silently during normal operation.
    EmergencyWithdraw { denom: String },

    /// Nominate a new guardian (step 1 of 2, guardian only).
    ProposeGuardian { new_guardian: String },

    /// Accept the guardian role (step 2 of 2, pending guardian only).
    AcceptGuardian {},
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    /// Returns the contract configuration
    #[returns(ConfigResponse)]
    Config {},

    /// Returns the next sequence number to be assigned
    #[returns(SequenceResponse)]
    Sequence {},

    /// Returns whether an inbound message has been executed
    #[returns(MessageStatusResponse)]
    MessageStatus { source_chain: u64, nonce: u64 },

    /// Returns whether the contract is paused
    #[returns(PausedResponse)]
    Paused {},
}

#[cw_serde]
pub struct ConfigResponse {
    pub guardian: String,
    pub pending_guardian: Option<String>,
    pub hub_chain_id: u16,
    pub fee_rate_bps: u16,
}

#[cw_serde]
pub struct SequenceResponse {
    pub sequence: u64,
}

#[cw_serde]
pub struct MessageStatusResponse {
    pub source_chain: u64,
    pub nonce: u64,
    pub executed: bool,
}

#[cw_serde]
pub struct PausedResponse {
    pub paused: bool,
}
