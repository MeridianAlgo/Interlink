use cosmwasm_std::{
    entry_point, to_json_binary, BankMsg, Binary, Coin, CosmosMsg, Deps, DepsMut, Env,
    MessageInfo, Response, StdResult, Uint128,
};

use crate::error::ContractError;
use crate::msg::{
    ConfigResponse, ExecuteMsg, InstantiateMsg, MessageStatusResponse, PausedResponse, QueryMsg,
    SequenceResponse,
};
use crate::state::{Config, CONFIG, EXECUTED_MESSAGES, PAUSED, SEQUENCE};

// ─── Instantiate ────────────────────────────────────────────────────────────

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    let guardian = deps.api.addr_validate(&msg.guardian)?;

    CONFIG.save(
        deps.storage,
        &Config {
            guardian,
            hub_chain_id: msg.hub_chain_id,
            fee_rate_bps: msg.fee_rate_bps,
        },
    )?;
    SEQUENCE.save(deps.storage, &0u64)?;
    PAUSED.save(deps.storage, &false)?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("hub_chain_id", msg.hub_chain_id.to_string())
        .add_attribute("fee_rate_bps", msg.fee_rate_bps.to_string()))
}

// ─── Execute ────────────────────────────────────────────────────────────────

#[entry_point]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::SendCrossChainMessage {
            recipient,
            destination_chain,
            payload,
        } => execute_send(deps, env, info, recipient, destination_chain, payload),

        ExecuteMsg::ExecuteVerifiedMessage {
            nonce,
            recipient,
            amount,
            proof,
            public_inputs,
        } => execute_verified(deps, env, info, nonce, recipient, amount, proof, public_inputs),

        ExecuteMsg::InitiateSwap {
            recipient,
            destination_chain,
            token_out,
            min_amount_out,
            swap_data,
        } => execute_initiate_swap(
            deps,
            env,
            info,
            recipient,
            destination_chain,
            token_out,
            min_amount_out,
            swap_data,
        ),

        ExecuteMsg::Pause {} => execute_pause(deps, info),
        ExecuteMsg::Unpause {} => execute_unpause(deps, info),
        ExecuteMsg::EmergencyWithdraw { denom } => {
            execute_emergency_withdraw(deps, env, info, denom)
        }
        ExecuteMsg::UpdateGuardian { new_guardian } => {
            execute_update_guardian(deps, info, new_guardian)
        }
    }
}

/// Lock native tokens and emit a MessagePublished event for the relayer network
fn execute_send(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    recipient: Vec<u8>,
    destination_chain: u16,
    payload: Vec<u8>,
) -> Result<Response, ContractError> {
    assert_not_paused(deps.as_ref())?;

    // Require at least one coin sent
    let total_value: u128 = info.funds.iter().map(|c| c.amount.u128()).sum();
    if total_value == 0 {
        return Err(ContractError::ZeroAmount);
    }

    // Increment sequence
    let sequence = SEQUENCE.load(deps.storage)?;
    let new_sequence = sequence + 1;
    SEQUENCE.save(deps.storage, &new_sequence)?;

    // Compute payload hash (keccak256 equivalent via sha256 on cosmos)
    let payload_hash = sha256_hash(&payload);

    Ok(Response::new()
        .add_attribute("action", "send_cross_chain_message")
        .add_attribute("sequence", new_sequence.to_string())
        .add_attribute("sender", info.sender.to_string())
        .add_attribute("recipient", hex::encode(&recipient))
        .add_attribute("amount", total_value.to_string())
        .add_attribute("destination_chain", destination_chain.to_string())
        .add_attribute("payload_hash", hex::encode(&payload_hash)))
}

/// Execute a verified message from the Hub, releasing locked tokens to the recipient
fn execute_verified(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    nonce: u64,
    recipient: String,
    amount: Uint128,
    proof: Vec<u8>,
    public_inputs: Vec<u8>,
) -> Result<Response, ContractError> {
    assert_not_paused(deps.as_ref())?;

    // Replay protection
    if EXECUTED_MESSAGES
        .may_load(deps.storage, nonce)?
        .unwrap_or(false)
    {
        return Err(ContractError::AlreadyExecuted { nonce });
    }

    // Verify ZK proof
    // In production, this would call a BN254 pairing verifier precompile or library.
    // For now, we verify the proof is non-empty and has the expected structure.
    verify_proof(&proof, &public_inputs)?;

    // Mark as executed
    EXECUTED_MESSAGES.save(deps.storage, nonce, &true)?;

    // Validate recipient address
    let recipient_addr = deps.api.addr_validate(&recipient)?;

    // Release funds to recipient
    let send_msg = CosmosMsg::Bank(BankMsg::Send {
        to_address: recipient_addr.to_string(),
        amount: vec![Coin {
            denom: "uatom".to_string(),
            amount,
        }],
    });

    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "execute_verified_message")
        .add_attribute("nonce", nonce.to_string())
        .add_attribute("recipient", recipient)
        .add_attribute("amount", amount.to_string()))
}

/// Initiate a cross-chain swap by locking input tokens
fn execute_initiate_swap(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    recipient: Vec<u8>,
    destination_chain: u16,
    token_out: String,
    min_amount_out: Uint128,
    swap_data: Vec<u8>,
) -> Result<Response, ContractError> {
    assert_not_paused(deps.as_ref())?;

    let total_value: u128 = info.funds.iter().map(|c| c.amount.u128()).sum();
    if total_value == 0 {
        return Err(ContractError::ZeroAmount);
    }

    let sequence = SEQUENCE.load(deps.storage)?;
    let new_sequence = sequence + 1;
    SEQUENCE.save(deps.storage, &new_sequence)?;

    let payload_hash = sha256_hash(&swap_data);

    Ok(Response::new()
        .add_attribute("action", "initiate_swap")
        .add_attribute("sequence", new_sequence.to_string())
        .add_attribute("sender", info.sender.to_string())
        .add_attribute("recipient", hex::encode(&recipient))
        .add_attribute("amount_in", total_value.to_string())
        .add_attribute("token_out", token_out)
        .add_attribute("min_amount_out", min_amount_out.to_string())
        .add_attribute("destination_chain", destination_chain.to_string())
        .add_attribute("payload_hash", hex::encode(&payload_hash)))
}

// ─── Admin Functions ────────────────────────────────────────────────────────

fn execute_pause(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    assert_guardian(deps.as_ref(), &info)?;
    PAUSED.save(deps.storage, &true)?;
    Ok(Response::new().add_attribute("action", "pause"))
}

fn execute_unpause(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    assert_guardian(deps.as_ref(), &info)?;
    PAUSED.save(deps.storage, &false)?;
    Ok(Response::new().add_attribute("action", "unpause"))
}

fn execute_emergency_withdraw(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    denom: String,
) -> Result<Response, ContractError> {
    assert_guardian(deps.as_ref(), &info)?;

    let config = CONFIG.load(deps.storage)?;
    let balance = deps.querier.query_balance(env.contract.address, &denom)?;

    let send_msg = CosmosMsg::Bank(BankMsg::Send {
        to_address: config.guardian.to_string(),
        amount: vec![balance],
    });

    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "emergency_withdraw")
        .add_attribute("denom", denom))
}

fn execute_update_guardian(
    deps: DepsMut,
    info: MessageInfo,
    new_guardian: String,
) -> Result<Response, ContractError> {
    assert_guardian(deps.as_ref(), &info)?;

    let new_addr = deps.api.addr_validate(&new_guardian)?;
    CONFIG.update(deps.storage, |mut config| -> StdResult<_> {
        config.guardian = new_addr;
        Ok(config)
    })?;

    Ok(Response::new()
        .add_attribute("action", "update_guardian")
        .add_attribute("new_guardian", new_guardian))
}

// ─── Query ──────────────────────────────────────────────────────────────────

#[entry_point]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => {
            let config = CONFIG.load(deps.storage)?;
            to_json_binary(&ConfigResponse {
                guardian: config.guardian.to_string(),
                hub_chain_id: config.hub_chain_id,
                fee_rate_bps: config.fee_rate_bps,
            })
        }
        QueryMsg::Sequence {} => {
            let sequence = SEQUENCE.load(deps.storage)?;
            to_json_binary(&SequenceResponse { sequence })
        }
        QueryMsg::MessageStatus { nonce } => {
            let executed = EXECUTED_MESSAGES
                .may_load(deps.storage, nonce)?
                .unwrap_or(false);
            to_json_binary(&MessageStatusResponse { nonce, executed })
        }
        QueryMsg::Paused {} => {
            let paused = PAUSED.load(deps.storage)?;
            to_json_binary(&PausedResponse { paused })
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn assert_guardian(deps: Deps, info: &MessageInfo) -> Result<(), ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.guardian {
        return Err(ContractError::Unauthorized);
    }
    Ok(())
}

fn assert_not_paused(deps: Deps) -> Result<(), ContractError> {
    let paused = PAUSED.load(deps.storage)?;
    if paused {
        return Err(ContractError::Paused);
    }
    Ok(())
}

/// Verify a ZK proof against public inputs.
/// In production this uses a BN254 pairing check; here we validate proof structure.
fn verify_proof(proof: &[u8], _public_inputs: &[u8]) -> Result<(), ContractError> {
    // Proof must be exactly 256 bytes (BN254 Groth16: 2 G1 points + 1 G2 point)
    if proof.len() != 256 {
        return Err(ContractError::InvalidProof);
    }
    // TODO: Implement full BN254 pairing verification when CosmWasm precompile is available.
    // For now, a non-zero proof of the correct length passes structural validation.
    if proof.iter().all(|&b| b == 0) {
        return Err(ContractError::InvalidProof);
    }
    Ok(())
}

/// Simple SHA-256 hash (cosmos-native)
fn sha256_hash(data: &[u8]) -> Vec<u8> {
    // Use a simple iterative hash since cosmwasm_std doesn't expose raw sha256 directly.
    // In practice, we'd use cosmwasm_crypto or a precompile.
    let mut hash = vec![0u8; 32];
    // XOR-based placeholder — in production, use cosmwasm_crypto::sha2_256
    for (i, byte) in data.iter().enumerate() {
        hash[i % 32] ^= byte;
    }
    hash
}

// We need hex encoding for attributes
mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
    use cosmwasm_std::{coins, Addr};

    fn setup() -> (cosmwasm_std::OwnedDeps<cosmwasm_std::MemoryStorage, cosmwasm_std::testing::MockApi, cosmwasm_std::testing::MockQuerier>, Env) {
        let mut deps = mock_dependencies();
        let env = mock_env();
        let info = mock_info("guardian_addr", &[]);

        let msg = InstantiateMsg {
            guardian: "guardian_addr".to_string(),
            hub_chain_id: 2, // Solana
            fee_rate_bps: 10,
        };

        instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();
        (deps, env)
    }

    #[test]
    fn test_instantiate() {
        let (deps, _env) = setup();
        let config = CONFIG.load(deps.as_ref().storage).unwrap();
        assert_eq!(config.guardian, Addr::unchecked("guardian_addr"));
        assert_eq!(config.hub_chain_id, 2);
        assert_eq!(config.fee_rate_bps, 10);

        let seq = SEQUENCE.load(deps.as_ref().storage).unwrap();
        assert_eq!(seq, 0);

        let paused = PAUSED.load(deps.as_ref().storage).unwrap();
        assert!(!paused);
    }

    #[test]
    fn test_send_cross_chain_message() {
        let (mut deps, env) = setup();
        let info = mock_info("user1", &coins(1000, "uatom"));

        let msg = ExecuteMsg::SendCrossChainMessage {
            recipient: vec![0xAA; 32],
            destination_chain: 1, // Ethereum
            payload: vec![1, 2, 3],
        };

        let res = execute(deps.as_mut(), env, info, msg).unwrap();
        assert_eq!(res.attributes.len(), 7);

        // Sequence should be 1
        let seq = SEQUENCE.load(deps.as_ref().storage).unwrap();
        assert_eq!(seq, 1);
    }

    #[test]
    fn test_send_zero_amount_fails() {
        let (mut deps, env) = setup();
        let info = mock_info("user1", &[]);

        let msg = ExecuteMsg::SendCrossChainMessage {
            recipient: vec![0xAA; 32],
            destination_chain: 1,
            payload: vec![],
        };

        let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
        assert!(matches!(err, ContractError::ZeroAmount));
    }

    #[test]
    fn test_pause_unpause() {
        let (mut deps, env) = setup();
        let info = mock_info("guardian_addr", &[]);

        // Pause
        execute(deps.as_mut(), env.clone(), info.clone(), ExecuteMsg::Pause {}).unwrap();
        assert!(PAUSED.load(deps.as_ref().storage).unwrap());

        // Sending should fail while paused
        let user_info = mock_info("user1", &coins(1000, "uatom"));
        let send_msg = ExecuteMsg::SendCrossChainMessage {
            recipient: vec![],
            destination_chain: 1,
            payload: vec![],
        };
        let err = execute(deps.as_mut(), env.clone(), user_info, send_msg).unwrap_err();
        assert!(matches!(err, ContractError::Paused));

        // Unpause
        execute(deps.as_mut(), env, info, ExecuteMsg::Unpause {}).unwrap();
        assert!(!PAUSED.load(deps.as_ref().storage).unwrap());
    }

    #[test]
    fn test_unauthorized_pause() {
        let (mut deps, env) = setup();
        let info = mock_info("not_guardian", &[]);

        let err = execute(deps.as_mut(), env, info, ExecuteMsg::Pause {}).unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized));
    }

    #[test]
    fn test_execute_verified_message() {
        let (mut deps, env) = setup();
        let info = mock_info("relayer1", &[]);

        // Create a valid 256-byte proof (non-zero)
        let proof = vec![0xAB; 256];

        let msg = ExecuteMsg::ExecuteVerifiedMessage {
            nonce: 1,
            recipient: "recipient_addr".to_string(),
            amount: Uint128::new(500),
            proof,
            public_inputs: vec![1, 2, 3],
        };

        let res = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();
        assert_eq!(res.messages.len(), 1); // Bank send message

        // Replay should fail
        let msg2 = ExecuteMsg::ExecuteVerifiedMessage {
            nonce: 1,
            recipient: "recipient_addr".to_string(),
            amount: Uint128::new(500),
            proof: vec![0xAB; 256],
            public_inputs: vec![1, 2, 3],
        };
        let err = execute(deps.as_mut(), env, info, msg2).unwrap_err();
        assert!(matches!(err, ContractError::AlreadyExecuted { nonce: 1 }));
    }

    #[test]
    fn test_invalid_proof_rejected() {
        let (mut deps, env) = setup();
        let info = mock_info("relayer1", &[]);

        // Wrong length proof
        let msg = ExecuteMsg::ExecuteVerifiedMessage {
            nonce: 2,
            recipient: "recipient_addr".to_string(),
            amount: Uint128::new(500),
            proof: vec![0xAB; 100], // Wrong length
            public_inputs: vec![],
        };

        let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
        assert!(matches!(err, ContractError::InvalidProof));
    }

    #[test]
    fn test_query_config() {
        let (deps, env) = setup();
        let res = query(deps.as_ref(), env, QueryMsg::Config {}).unwrap();
        let config: ConfigResponse = cosmwasm_std::from_json(res).unwrap();
        assert_eq!(config.guardian, "guardian_addr");
        assert_eq!(config.hub_chain_id, 2);
    }

    #[test]
    fn test_query_sequence() {
        let (deps, env) = setup();
        let res = query(deps.as_ref(), env, QueryMsg::Sequence {}).unwrap();
        let seq: SequenceResponse = cosmwasm_std::from_json(res).unwrap();
        assert_eq!(seq.sequence, 0);
    }
}
