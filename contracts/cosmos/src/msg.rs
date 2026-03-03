use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::Uint128;

#[cw_serde]
pub struct InstantiateMsg {
    /// The DAO guardian address that can pause/unpause and perform emergency actions
    pub guardian: String,
    /// The chain ID of the Solana Hub (destination for proofs)
    pub hub_chain_id: u16,
    /// Fee rate in basis points (e.g., 10 = 0.1%)
    pub fee_rate_bps: u16,
}

#[cw_serde]
pub enum ExecuteMsg {
    /// Send a cross-chain message by locking native tokens in the gateway vault.
    /// The relayer network will observe the resulting event and generate a ZK proof.
    SendCrossChainMessage {
        recipient: Vec<u8>,
        destination_chain: u16,
        payload: Vec<u8>,
    },

    /// Execute a verified message from the Hub. Called by relayers with a valid proof.
    /// Releases locked tokens to the specified recipient.
    ExecuteVerifiedMessage {
        nonce: u64,
        recipient: String,
        amount: Uint128,
        proof: Vec<u8>,
        public_inputs: Vec<u8>,
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

    /// Emergency withdraw all funds to the guardian (guardian only)
    EmergencyWithdraw { denom: String },

    /// Update the guardian address (guardian only)
    UpdateGuardian { new_guardian: String },
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    /// Returns the contract configuration
    #[returns(ConfigResponse)]
    Config {},

    /// Returns the current sequence number
    #[returns(SequenceResponse)]
    Sequence {},

    /// Returns whether a message nonce has been executed
    #[returns(MessageStatusResponse)]
    MessageStatus { nonce: u64 },

    /// Returns whether the contract is paused
    #[returns(PausedResponse)]
    Paused {},
}

#[cw_serde]
pub struct ConfigResponse {
    pub guardian: String,
    pub hub_chain_id: u16,
    pub fee_rate_bps: u16,
}

#[cw_serde]
pub struct SequenceResponse {
    pub sequence: u64,
}

#[cw_serde]
pub struct MessageStatusResponse {
    pub nonce: u64,
    pub executed: bool,
}

#[cw_serde]
pub struct PausedResponse {
    pub paused: bool,
}
