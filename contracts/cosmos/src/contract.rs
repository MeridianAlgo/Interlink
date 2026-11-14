use cosmwasm_std::{
    entry_point, to_json_binary, Addr, BankMsg, Binary, Coin, CosmosMsg, Deps, DepsMut, Env,
    MessageInfo, Response, StdResult, Uint128,
};
use sha2::{Digest, Sha256};

use crate::error::ContractError;
use crate::msg::{
    ConfigResponse, ExecuteMsg, InstantiateMsg, MessageStatusResponse, PausedResponse, QueryMsg,
    SequenceResponse,
};
use crate::state::{Config, CONFIG, EXECUTED_MESSAGES, PAUSED, PENDING_GUARDIAN, SEQUENCE};

/// Upper bound on the bridge fee (10%). Prevents a misconfigured instance
/// from confiscating most of every transfer.
pub const MAX_FEE_RATE_BPS: u16 = 1_000;

/// Domain tags so a hash computed for one purpose can never be reused for another.
const OUTBOUND_DOMAIN: &[u8] = b"interlink_v1/cosmos/outbound";
const INBOUND_DOMAIN: &[u8] = b"interlink_v1/cosmos/inbound";

// ─── Instantiate ────────────────────────────────────────────────────────────

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    if msg.fee_rate_bps > MAX_FEE_RATE_BPS {
        return Err(ContractError::InvalidFeeRate {
            fee_rate_bps: msg.fee_rate_bps,
            max: MAX_FEE_RATE_BPS,
        });
    }
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
            source_chain,
            nonce,
            recipient,
            denom,
            amount,
            proof,
        } => execute_verified(
            deps,
            env,
            source_chain,
            nonce,
            recipient,
            denom,
            amount,
            proof,
        ),

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
        ExecuteMsg::ProposeGuardian { new_guardian } => {
            execute_propose_guardian(deps, info, new_guardian)
        }
        ExecuteMsg::AcceptGuardian {} => execute_accept_guardian(deps, info),
    }
}

/// Lock exactly one native coin and emit a MessagePublished event for the relayer network.
fn execute_send(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    recipient: Vec<u8>,
    destination_chain: u16,
    payload: Vec<u8>,
) -> Result<Response, ContractError> {
    assert_not_paused(deps.as_ref())?;
    validate_destination(destination_chain)?;
    let coin = one_coin(&info)?;

    let sequence = next_sequence(deps.storage)?;
    let payload_hash = outbound_hash(
        &env,
        sequence,
        &info.sender,
        &recipient,
        &coin,
        destination_chain,
        &[b"send", &Sha256::digest(&payload)],
    );

    Ok(Response::new()
        .add_attribute("action", "send_cross_chain_message")
        .add_attribute("sequence", sequence.to_string())
        .add_attribute("sender", info.sender.to_string())
        .add_attribute("recipient", hex::encode(&recipient))
        .add_attribute("denom", coin.denom)
        .add_attribute("amount", coin.amount.to_string())
        .add_attribute("destination_chain", destination_chain.to_string())
        .add_attribute("payload_hash", hex::encode(&payload_hash)))
}

/// Execute a verified message from the Hub, releasing locked tokens to the recipient.
#[allow(clippy::too_many_arguments)]
fn execute_verified(
    deps: DepsMut,
    env: Env,
    source_chain: u64,
    nonce: u64,
    recipient: String,
    denom: String,
    amount: Uint128,
    proof: Vec<u8>,
) -> Result<Response, ContractError> {
    assert_not_paused(deps.as_ref())?;

    if EXECUTED_MESSAGES
        .may_load(deps.storage, (source_chain, nonce))?
        .unwrap_or(false)
    {
        return Err(ContractError::AlreadyExecuted {
            source_chain,
            nonce,
        });
    }
    if amount.is_zero() {
        return Err(ContractError::ZeroAmount);
    }
    let recipient_addr = deps.api.addr_validate(&recipient)?;

    // Everything that decides where funds go is hashed here, on-chain, and
    // becomes the proof's public input. The caller cannot substitute values.
    let payload_hash = inbound_hash(&env, source_chain, nonce, &recipient_addr, &denom, amount);
    verify_proof(&proof, &payload_hash, nonce)?;

    EXECUTED_MESSAGES.save(deps.storage, (source_chain, nonce), &true)?;

    let config = CONFIG.load(deps.storage)?;
    let fee_amount = amount.multiply_ratio(config.fee_rate_bps as u128, 10_000u128);
    let release_amount = amount - fee_amount;

    let send_msg = CosmosMsg::Bank(BankMsg::Send {
        to_address: recipient_addr.to_string(),
        amount: vec![Coin {
            denom: denom.clone(),
            amount: release_amount,
        }],
    });

    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "execute_verified_message")
        .add_attribute("source_chain", source_chain.to_string())
        .add_attribute("nonce", nonce.to_string())
        .add_attribute("recipient", recipient)
        .add_attribute("denom", denom)
        .add_attribute("amount", release_amount.to_string())
        .add_attribute("fee", fee_amount.to_string()))
}

/// Initiate a cross-chain swap by locking exactly one input coin.
#[allow(clippy::too_many_arguments)]
fn execute_initiate_swap(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    recipient: Vec<u8>,
    destination_chain: u16,
    token_out: String,
    min_amount_out: Uint128,
    swap_data: Vec<u8>,
) -> Result<Response, ContractError> {
    assert_not_paused(deps.as_ref())?;
    validate_destination(destination_chain)?;
    let coin = one_coin(&info)?;

    let sequence = next_sequence(deps.storage)?;
    let payload_hash = outbound_hash(
        &env,
        sequence,
        &info.sender,
        &recipient,
        &coin,
        destination_chain,
        &[
            b"swap",
            token_out.as_bytes(),
            &min_amount_out.u128().to_be_bytes(),
            &Sha256::digest(&swap_data),
        ],
    );

    Ok(Response::new()
        .add_attribute("action", "initiate_swap")
        .add_attribute("sequence", sequence.to_string())
        .add_attribute("sender", info.sender.to_string())
        .add_attribute("recipient", hex::encode(&recipient))
        .add_attribute("denom", coin.denom)
        .add_attribute("amount_in", coin.amount.to_string())
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
    if !PAUSED.load(deps.storage)? {
        return Err(ContractError::NotPaused);
    }

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

fn execute_propose_guardian(
    deps: DepsMut,
    info: MessageInfo,
    new_guardian: String,
) -> Result<Response, ContractError> {
    assert_guardian(deps.as_ref(), &info)?;
    let new_addr = deps.api.addr_validate(&new_guardian)?;
    PENDING_GUARDIAN.save(deps.storage, &new_addr)?;

    Ok(Response::new()
        .add_attribute("action", "propose_guardian")
        .add_attribute("pending_guardian", new_guardian))
}

fn execute_accept_guardian(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let pending = PENDING_GUARDIAN
        .may_load(deps.storage)?
        .ok_or(ContractError::NoPendingGuardian)?;
    if info.sender != pending {
        return Err(ContractError::Unauthorized);
    }
    CONFIG.update(deps.storage, |mut config| -> StdResult<_> {
        config.guardian = pending.clone();
        Ok(config)
    })?;
    PENDING_GUARDIAN.remove(deps.storage);

    Ok(Response::new()
        .add_attribute("action", "accept_guardian")
        .add_attribute("guardian", pending.to_string()))
}

// ─── Query ──────────────────────────────────────────────────────────────────

#[entry_point]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => {
            let config = CONFIG.load(deps.storage)?;
            let pending = PENDING_GUARDIAN.may_load(deps.storage)?;
            to_json_binary(&ConfigResponse {
                guardian: config.guardian.to_string(),
                pending_guardian: pending.map(|a| a.to_string()),
                hub_chain_id: config.hub_chain_id,
                fee_rate_bps: config.fee_rate_bps,
            })
        }
        QueryMsg::Sequence {} => {
            let sequence = SEQUENCE.load(deps.storage)?;
            to_json_binary(&SequenceResponse { sequence })
        }
        QueryMsg::MessageStatus {
            source_chain,
            nonce,
        } => {
            let executed = EXECUTED_MESSAGES
                .may_load(deps.storage, (source_chain, nonce))?
                .unwrap_or(false);
            to_json_binary(&MessageStatusResponse {
                source_chain,
                nonce,
                executed,
            })
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
    if PAUSED.load(deps.storage)? {
        return Err(ContractError::Paused);
    }
    Ok(())
}

fn validate_destination(destination_chain: u16) -> Result<(), ContractError> {
    if destination_chain == 0 {
        return Err(ContractError::InvalidDestinationChain {
            chain_id: destination_chain,
        });
    }
    Ok(())
}

/// Require exactly one non-zero coin. The previous version summed amounts
/// across *different* denoms, so 1 ATOM + 1,000 worthless IBC tokens was
/// reported as "1001" with no denom, letting the other side mint the wrong asset.
fn one_coin(info: &MessageInfo) -> Result<Coin, ContractError> {
    match info.funds.as_slice() {
        [] => Err(ContractError::ZeroAmount),
        [coin] if coin.amount.is_zero() => Err(ContractError::ZeroAmount),
        [coin] => Ok(coin.clone()),
        _ => Err(ContractError::MultipleDenoms),
    }
}

/// Return the current sequence and advance the counter.
fn next_sequence(storage: &mut dyn cosmwasm_std::Storage) -> StdResult<u64> {
    let sequence = SEQUENCE.load(storage)?;
    SEQUENCE.save(storage, &(sequence + 1))?;
    Ok(sequence)
}

/// SHA-256 over length-prefixed fields. Length prefixes stop two different
/// field lists from concatenating to the same bytes.
fn hash_fields(fields: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update((field.len() as u32).to_be_bytes());
        hasher.update(field);
    }
    hasher.finalize().into()
}

/// Commitment to an outbound message. Binds this chain and contract so the
/// same event cannot be replayed as if it came from another deployment.
fn outbound_hash(
    env: &Env,
    sequence: u64,
    sender: &Addr,
    recipient: &[u8],
    coin: &Coin,
    destination_chain: u16,
    extra: &[&[u8]],
) -> [u8; 32] {
    let mut fields: Vec<&[u8]> = vec![OUTBOUND_DOMAIN];
    let seq = sequence.to_be_bytes();
    let amount = coin.amount.u128().to_be_bytes();
    let dest = destination_chain.to_be_bytes();
    fields.extend_from_slice(&[
        env.block.chain_id.as_bytes(),
        env.contract.address.as_bytes(),
        &seq,
        sender.as_bytes(),
        recipient,
        coin.denom.as_bytes(),
        &amount,
        &dest,
    ]);
    fields.extend_from_slice(extra);
    hash_fields(&fields)
}

/// Commitment to an inbound message. This is what the proof must attest to.
fn inbound_hash(
    env: &Env,
    source_chain: u64,
    nonce: u64,
    recipient: &Addr,
    denom: &str,
    amount: Uint128,
) -> [u8; 32] {
    hash_fields(&[
        INBOUND_DOMAIN,
        env.block.chain_id.as_bytes(),
        env.contract.address.as_bytes(),
        &source_chain.to_be_bytes(),
        &nonce.to_be_bytes(),
        recipient.as_bytes(),
        denom.as_bytes(),
        &amount.u128().to_be_bytes(),
    ])
}

/// Verify a proof that `payload_hash` was emitted by the source chain.
///
/// **This always fails, on purpose.** The previous implementation accepted any
/// 256-byte blob that was not all zeros, which let anyone call
/// `ExecuteVerifiedMessage` with `[0xAB; 256]` and drain every denom held by
/// the contract.
///
/// CosmWasm 1.5 has no BN254 pairing host function, and a pure-Wasm pairing
/// costs far more gas than a block allows. Options for a real verifier:
///   - Re-target the proof system to BLS12-381 and use the native
///     `bls12_381_pairing_equality` host function (CosmWasm 2.1+).
///   - Deploy on a chain with a BN254 precompile/module and call it here.
///   - Verify on the Solana hub and relay to Cosmos over IBC with a light client.
///
/// Until one of those exists, inbound messages stay disabled.
fn verify_proof(_proof: &[u8], _payload_hash: &[u8; 32], _nonce: u64) -> Result<(), ContractError> {
    Err(ContractError::VerifierUnavailable)
}

mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
    use cosmwasm_std::{coin, coins, Addr};

    type Deps = cosmwasm_std::OwnedDeps<
        cosmwasm_std::MemoryStorage,
        cosmwasm_std::testing::MockApi,
        cosmwasm_std::testing::MockQuerier,
    >;

    fn setup() -> (Deps, Env) {
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

    fn attr<'a>(res: &'a Response, key: &str) -> &'a str {
        &res.attributes.iter().find(|a| a.key == key).unwrap().value
    }

    #[test]
    fn test_instantiate() {
        let (deps, _env) = setup();
        let config = CONFIG.load(deps.as_ref().storage).unwrap();
        assert_eq!(config.guardian, Addr::unchecked("guardian_addr"));
        assert_eq!(config.hub_chain_id, 2);
        assert_eq!(config.fee_rate_bps, 10);
        assert_eq!(SEQUENCE.load(deps.as_ref().storage).unwrap(), 0);
        assert!(!PAUSED.load(deps.as_ref().storage).unwrap());
    }

    #[test]
    fn test_instantiate_rejects_excessive_fee() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            guardian: "guardian_addr".to_string(),
            hub_chain_id: 2,
            fee_rate_bps: MAX_FEE_RATE_BPS + 1,
        };
        let err = instantiate(deps.as_mut(), mock_env(), mock_info("x", &[]), msg).unwrap_err();
        assert!(matches!(err, ContractError::InvalidFeeRate { .. }));
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
        // First message is sequence 0 (matches EVM gateway); counter advances to 1.
        assert_eq!(attr(&res, "sequence"), "0");
        assert_eq!(attr(&res, "denom"), "uatom");
        assert_eq!(attr(&res, "amount"), "1000");
        assert_eq!(SEQUENCE.load(deps.as_ref().storage).unwrap(), 1);
    }

    #[test]
    fn test_send_rejects_multiple_denoms() {
        let (mut deps, env) = setup();
        // Previously summed to "1001" with no denom recorded.
        let info = mock_info("user1", &[coin(1, "uatom"), coin(1000, "ibc/WORTHLESS")]);
        let msg = ExecuteMsg::SendCrossChainMessage {
            recipient: vec![0xAA; 32],
            destination_chain: 1,
            payload: vec![],
        };
        let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
        assert!(matches!(err, ContractError::MultipleDenoms));
    }

    #[test]
    fn test_payload_hash_binds_amount_and_denom() {
        let (mut deps, env) = setup();
        let send = |deps: &mut Deps, funds: Vec<Coin>| {
            let msg = ExecuteMsg::SendCrossChainMessage {
                recipient: vec![0xAA; 32],
                destination_chain: 1,
                payload: vec![1, 2, 3],
            };
            let res = execute(deps.as_mut(), env.clone(), mock_info("user1", &funds), msg).unwrap();
            attr(&res, "payload_hash").to_string()
        };
        let h1 = send(&mut deps, coins(1000, "uatom"));
        let h2 = send(&mut deps, coins(1000, "uosmo"));
        // Different denoms (and sequences) must never produce the same commitment.
        assert_ne!(h1, h2);
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

        execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            ExecuteMsg::Pause {},
        )
        .unwrap();
        assert!(PAUSED.load(deps.as_ref().storage).unwrap());

        let user_info = mock_info("user1", &coins(1000, "uatom"));
        let send_msg = ExecuteMsg::SendCrossChainMessage {
            recipient: vec![],
            destination_chain: 1,
            payload: vec![],
        };
        let err = execute(deps.as_mut(), env.clone(), user_info, send_msg).unwrap_err();
        assert!(matches!(err, ContractError::Paused));

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

    /// Regression test for the critical drain: the old verifier accepted any
    /// non-zero 256-byte blob, so this exact call released funds.
    #[test]
    fn test_execute_verified_message_rejects_forged_proof() {
        let (mut deps, env) = setup();
        let msg = ExecuteMsg::ExecuteVerifiedMessage {
            source_chain: 1,
            nonce: 1,
            recipient: "attacker".to_string(),
            denom: "uatom".to_string(),
            amount: Uint128::new(500),
            proof: vec![0xAB; 256],
        };
        let err = execute(deps.as_mut(), env, mock_info("anyone", &[]), msg).unwrap_err();
        assert!(matches!(err, ContractError::VerifierUnavailable));

        // Nothing was marked executed.
        assert!(EXECUTED_MESSAGES
            .may_load(deps.as_ref().storage, (1, 1))
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_replay_is_scoped_per_source_chain() {
        let (mut deps, env) = setup();
        EXECUTED_MESSAGES
            .save(deps.as_mut().storage, (1, 7), &true)
            .unwrap();

        let msg = |source_chain| ExecuteMsg::ExecuteVerifiedMessage {
            source_chain,
            nonce: 7,
            recipient: "recipient_addr".to_string(),
            denom: "uatom".to_string(),
            amount: Uint128::new(500),
            proof: vec![0xAB; 256],
        };

        let err = execute(deps.as_mut(), env.clone(), mock_info("r", &[]), msg(1)).unwrap_err();
        assert!(matches!(
            err,
            ContractError::AlreadyExecuted {
                source_chain: 1,
                nonce: 7
            }
        ));

        // Same nonce from another chain is not blocked by the replay guard
        // (it fails later only because no verifier is configured).
        let err = execute(deps.as_mut(), env, mock_info("r", &[]), msg(2)).unwrap_err();
        assert!(matches!(err, ContractError::VerifierUnavailable));
    }

    #[test]
    fn test_emergency_withdraw_requires_pause() {
        let (mut deps, env) = setup();
        let guardian = mock_info("guardian_addr", &[]);
        let withdraw = || ExecuteMsg::EmergencyWithdraw {
            denom: "uatom".to_string(),
        };

        let err = execute(deps.as_mut(), env.clone(), guardian.clone(), withdraw()).unwrap_err();
        assert!(matches!(err, ContractError::NotPaused));

        execute(
            deps.as_mut(),
            env.clone(),
            guardian.clone(),
            ExecuteMsg::Pause {},
        )
        .unwrap();
        execute(deps.as_mut(), env, guardian, withdraw()).unwrap();
    }

    #[test]
    fn test_two_step_guardian_transfer() {
        let (mut deps, env) = setup();
        let propose = ExecuteMsg::ProposeGuardian {
            new_guardian: "new_guardian".to_string(),
        };
        execute(
            deps.as_mut(),
            env.clone(),
            mock_info("guardian_addr", &[]),
            propose,
        )
        .unwrap();

        // Still the old guardian until accepted.
        assert_eq!(
            CONFIG.load(deps.as_ref().storage).unwrap().guardian,
            Addr::unchecked("guardian_addr")
        );

        let err = execute(
            deps.as_mut(),
            env.clone(),
            mock_info("someone_else", &[]),
            ExecuteMsg::AcceptGuardian {},
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized));

        execute(
            deps.as_mut(),
            env,
            mock_info("new_guardian", &[]),
            ExecuteMsg::AcceptGuardian {},
        )
        .unwrap();
        assert_eq!(
            CONFIG.load(deps.as_ref().storage).unwrap().guardian,
            Addr::unchecked("new_guardian")
        );
    }

    #[test]
    fn test_query_config() {
        let (deps, env) = setup();
        let res = query(deps.as_ref(), env, QueryMsg::Config {}).unwrap();
        let config: ConfigResponse = cosmwasm_std::from_json(res).unwrap();
        assert_eq!(config.guardian, "guardian_addr");
        assert_eq!(config.hub_chain_id, 2);
        assert!(config.pending_guardian.is_none());
    }

    #[test]
    fn test_query_sequence() {
        let (deps, env) = setup();
        let res = query(deps.as_ref(), env, QueryMsg::Sequence {}).unwrap();
        let seq: SequenceResponse = cosmwasm_std::from_json(res).unwrap();
        assert_eq!(seq.sequence, 0);
    }
}
