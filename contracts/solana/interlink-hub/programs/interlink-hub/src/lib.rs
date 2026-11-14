use anchor_lang::prelude::*;
use anchor_lang::solana_program::alt_bn128::prelude::{
    alt_bn128_addition, alt_bn128_multiplication, alt_bn128_pairing,
};
use anchor_lang::solana_program::keccak;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{self, Burn, Mint, MintTo, Token, TokenAccount, Transfer};

declare_id!("AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz");

// ─── Protocol constants ─────────────────────────────────────────────────────

/// Minimum ILINK stake required for relayers (100,000 tokens with 6 decimals)
const MIN_STAKE_AMOUNT: u64 = 100_000_000_000;

/// Slashing percentage (50% of stake burned)
const SLASH_BPS: u64 = 5_000;

/// Upper bound on the swap fee (10%). Above 100% the old code panicked on every swap.
const MAX_FEE_RATE_BPS: u16 = 1_000;

/// Stake stays locked this long after a relayer's last attestation, so there
/// is a window to slash before the stake can be withdrawn. Without it a
/// relayer could stake, attest a fraudulent message and unstake in one tx.
pub const UNBONDING_PERIOD_SECS: i64 = 7 * 24 * 60 * 60;

/// Delay before a replacement verification key takes effect. Replacing the
/// VK is equivalent to replacing the bridge's security, so users get a
/// window to see it coming and exit.
pub const VK_TIMELOCK_SECS: i64 = 2 * 24 * 60 * 60;

/// This hub's chain ID in the canonical message format. The program ID is
/// also hashed into every message, so a chain-ID collision alone cannot be
/// used to replay messages between deployments.
pub const HUB_CHAIN_ID: u64 = 2;

/// Message actions (must match the EVM gateway).
pub const ACTION_TRANSFER: u8 = 1;
pub const ACTION_SWAP: u8 = 2;

/// Domain salt for the circuit's round constant (must match the prover).
const DOMAIN_SALT: &[u8] = b"interlink_v1_domain";

/// Type tag hashed into every canonical message (must match the EVM gateway).
const MESSAGE_TYPE_TAG: &[u8] = b"InterlinkMessage(v1)";

#[program]
pub mod interlink_hub {
    use super::*;

    /// One-time setup. Only the program's upgrade authority may call this,
    /// so nobody can front-run deployment and make themselves admin.
    pub fn initialize(ctx: Context<Initialize>, fee_rate_bps: u16) -> Result<()> {
        require!(fee_rate_bps <= MAX_FEE_RATE_BPS, HubError::InvalidFeeRate);

        let registry = &mut ctx.accounts.state_registry;
        registry.admin = ctx.accounts.admin.key();
        registry.ilink_mint = ctx.accounts.ilink_mint.key();
        registry.fee_rate_bps = fee_rate_bps;
        registry.total_staked = 0;
        registry.total_burned = 0;
        registry.vk_initialized = false;
        registry.outbound_nonce = 0;
        Ok(())
    }

    /// Record the trusted gateway on a remote chain. Set once per chain:
    /// changing where withdrawals are released is equivalent to changing who
    /// can unlock the bridge's collateral, so it is not editable in place.
    pub fn register_remote_gateway(
        ctx: Context<RegisterRemoteGateway>,
        chain_id: u64,
        gateway: [u8; 32],
    ) -> Result<()> {
        require!(gateway != [0u8; 32], HubError::InvalidRecipient);
        let remote = &mut ctx.accounts.remote_gateway;
        remote.chain_id = chain_id;
        remote.gateway = gateway;
        Ok(())
    }

    /// Burn wrapped tokens and emit an outbound message that releases the
    /// original asset on its source chain (`InterlinkGateway.releaseTokens`).
    pub fn burn_for_withdrawal(
        ctx: Context<BurnForWithdrawal>,
        recipient: [u8; 32],
        amount: u64,
    ) -> Result<()> {
        require!(amount > 0, HubError::AmountTooSmall);
        require!(recipient != [0u8; 32], HubError::InvalidRecipient);

        token::burn(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.token_mint.to_account_info(),
                    from: ctx.accounts.owner_token_account.to_account_info(),
                    authority: ctx.accounts.owner.to_account_info(),
                },
            ),
            amount,
        )?;

        let wrapped = &ctx.accounts.wrapped_asset;
        let source_amount = unscale_amount(amount, wrapped.decimals_shift)?;

        let registry = &mut ctx.accounts.state_registry;
        let nonce = registry.outbound_nonce;
        registry.outbound_nonce = nonce.checked_add(1).ok_or(HubError::MathOverflow)?;

        let dst_chain = wrapped.src_chain;
        let dst_gateway = ctx.accounts.remote_gateway.gateway;
        let sender = ctx.accounts.owner.key().to_bytes();
        let extra_hash = keccak::hash(&[]).to_bytes();
        let payload_hash = message_hash(&MessageFields {
            src_chain: HUB_CHAIN_ID,
            src_gateway: crate::ID.to_bytes(),
            dst_chain,
            dst_gateway,
            nonce,
            action: ACTION_TRANSFER,
            sender,
            asset: wrapped.asset,
            amount: source_amount,
            recipient,
            extra_hash,
        });

        emit!(OutboundMessageEvent {
            payload_hash,
            dst_chain,
            dst_gateway,
            nonce,
            action: ACTION_TRANSFER,
            sender,
            asset: wrapped.asset,
            amount: source_amount,
            recipient,
            extra_hash,
        });
        Ok(())
    }

    /// Store the initial Groth16 verification key. Can only be called once;
    /// later changes go through `propose_verification_key` + timelock.
    ///
    /// vk_data layout (576 bytes for 1 public input):
    ///   alpha_g1 (64) | beta_g2 (128) | gamma_g2 (128) | delta_g2 (128) | ic_0 (64) | ic_1 (64)
    pub fn set_verification_key(ctx: Context<SetVerificationKey>, vk_data: Vec<u8>) -> Result<()> {
        ctx.accounts.verification_key.key = VkBytes::try_from_slice_checked(&vk_data)?;
        ctx.accounts.state_registry.vk_initialized = true;

        emit!(VKUpdatedEvent {
            admin: ctx.accounts.admin.key(),
        });
        Ok(())
    }

    /// Queue a replacement verification key. Takes effect after `VK_TIMELOCK_SECS`.
    pub fn propose_verification_key(
        ctx: Context<ProposeVerificationKey>,
        vk_data: Vec<u8>,
    ) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let pending = &mut ctx.accounts.pending_verification_key;
        pending.key = VkBytes::try_from_slice_checked(&vk_data)?;
        pending.eta = now
            .checked_add(VK_TIMELOCK_SECS)
            .ok_or(HubError::MathOverflow)?;

        emit!(VKProposedEvent {
            admin: ctx.accounts.admin.key(),
            eta: pending.eta,
        });
        Ok(())
    }

    /// Activate a queued verification key once its timelock has elapsed.
    pub fn apply_verification_key(ctx: Context<ApplyVerificationKey>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let pending = &ctx.accounts.pending_verification_key;
        require!(now >= pending.eta, HubError::TimelockNotElapsed);

        ctx.accounts.verification_key.key = pending.key.clone();

        emit!(VKUpdatedEvent {
            admin: ctx.accounts.admin.key(),
        });
        Ok(())
    }

    /// Map a source-chain asset to the SPL mint that represents it here.
    /// `decimals_shift` = source decimals − mint decimals (e.g. 18 → 8 is 10).
    pub fn register_wrapped_asset(
        ctx: Context<RegisterWrappedAsset>,
        src_chain: u64,
        asset: [u8; 32],
        decimals_shift: u8,
    ) -> Result<()> {
        require!(decimals_shift <= 30, HubError::InvalidDecimalsShift);

        let wrapped = &mut ctx.accounts.wrapped_asset;
        wrapped.src_chain = src_chain;
        wrapped.asset = asset;
        wrapped.mint = ctx.accounts.token_mint.key();
        wrapped.decimals_shift = decimals_shift;
        Ok(())
    }

    /// Attest that a fully specified cross-chain message is proven.
    ///
    /// The payload hash is recomputed on-chain from the message fields, so the
    /// proof is bound to exactly these values. The record is keyed by the
    /// hash, which makes each message attestable once and leaves unrelated
    /// messages independent (no global sequence that one stuck message can
    /// block forever, and no way to consume another message's sequence).
    pub fn submit_proof(
        ctx: Context<SubmitProof>,
        payload_hash: [u8; 32],
        action: u8,
        msg: InboundMessage,
        proof_data: Vec<u8>,
    ) -> Result<()> {
        require!(
            ctx.accounts.stake_account.amount >= MIN_STAKE_AMOUNT,
            HubError::InsufficientStake
        );
        require!(
            ctx.accounts.state_registry.vk_initialized,
            HubError::VKNotInitialized
        );
        require!(
            action == ACTION_TRANSFER || action == ACTION_SWAP,
            HubError::WrongAction
        );
        require!(
            canonical_hash(action, &msg) == payload_hash,
            HubError::PayloadHashMismatch
        );
        require!(proof_data.len() == 256, HubError::InvalidProofLength);
        require!(
            verify_groth16_proof(
                &proof_data,
                &payload_hash,
                msg.nonce,
                &ctx.accounts.verification_key.key
            ),
            HubError::InvalidProof
        );

        let now = Clock::get()?.unix_timestamp;
        let record = &mut ctx.accounts.message_record;
        record.payload_hash = payload_hash;
        record.action = action;
        record.executed = false;
        record.attested_by = ctx.accounts.relayer.key();
        record.attested_at = now;

        // Restart the relayer's unbonding clock.
        ctx.accounts.stake_account.last_active = now;

        emit!(ProofVerifiedEvent {
            src_chain: msg.src_chain,
            sequence: msg.nonce,
            payload_hash,
            relayer: ctx.accounts.relayer.key(),
        });
        Ok(())
    }

    /// Mint wrapped tokens for an attested transfer. Permissionless: every
    /// value used here is bound to the attested payload hash, so the caller
    /// cannot change the amount, mint or recipient.
    pub fn execute_transfer(
        ctx: Context<ExecuteTransfer>,
        payload_hash: [u8; 32],
        msg: InboundMessage,
    ) -> Result<()> {
        require!(
            canonical_hash(ACTION_TRANSFER, &msg) == payload_hash,
            HubError::PayloadHashMismatch
        );

        let record = &mut ctx.accounts.message_record;
        require!(!record.executed, HubError::MessageAlreadyExecuted);
        require!(record.action == ACTION_TRANSFER, HubError::WrongAction);
        record.executed = true;

        let amount = scale_amount(msg.amount, ctx.accounts.wrapped_asset.decimals_shift)?;

        let seeds = &[b"mint_authority".as_ref(), &[ctx.bumps.mint_authority]];
        let signer_seeds = &[&seeds[..]];
        token::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint: ctx.accounts.token_mint.to_account_info(),
                    to: ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.mint_authority.to_account_info(),
                },
                signer_seeds,
            ),
            amount,
        )?;

        emit!(MintEvent {
            src_chain: msg.src_chain,
            sequence: msg.nonce,
            recipient: msg.recipient,
            amount,
            payload_hash,
        });
        Ok(())
    }

    /// Execute an attested cross-chain swap through the hub's pool.
    /// Like `execute_transfer`, every parameter is bound to the payload hash.
    pub fn execute_swap(
        ctx: Context<ExecuteSwap>,
        payload_hash: [u8; 32],
        msg: InboundMessage,
        token_out: Pubkey,
        min_amount_out: u64,
        swap_data_hash: [u8; 32],
    ) -> Result<()> {
        require!(
            swap_extra_hash(&token_out, min_amount_out, &swap_data_hash) == msg.extra_hash,
            HubError::PayloadHashMismatch
        );
        require!(
            canonical_hash(ACTION_SWAP, &msg) == payload_hash,
            HubError::PayloadHashMismatch
        );
        require!(
            ctx.accounts.liquidity_pool.token_b_mint == token_out,
            HubError::WrongToken
        );

        let record = &mut ctx.accounts.message_record;
        require!(!record.executed, HubError::MessageAlreadyExecuted);
        require!(record.action == ACTION_SWAP, HubError::WrongAction);
        record.executed = true;

        let amount_in = scale_amount(msg.amount, ctx.accounts.wrapped_asset.decimals_shift)?;
        let fee_rate_bps = ctx.accounts.state_registry.fee_rate_bps;

        let (amount_out, fee, token_a_mint, token_b_mint) = {
            let pool = &mut ctx.accounts.liquidity_pool;
            let reserve_in = pool.reserve_a;
            let reserve_out = pool.reserve_b;
            require!(reserve_in > 0 && reserve_out > 0, HubError::EmptyPool);

            let fee = mul_div(amount_in, fee_rate_bps as u64, 10_000)?;
            let amount_in_after_fee = amount_in.checked_sub(fee).ok_or(HubError::MathOverflow)?;

            // Constant product: out = in * R_out / (R_in + in)
            let denominator = (reserve_in as u128)
                .checked_add(amount_in_after_fee as u128)
                .ok_or(HubError::MathOverflow)?;
            let amount_out = u64::try_from(
                (amount_in_after_fee as u128)
                    .checked_mul(reserve_out as u128)
                    .ok_or(HubError::MathOverflow)?
                    / denominator,
            )
            .map_err(|_| HubError::MathOverflow)?;

            require!(amount_out >= min_amount_out, HubError::SlippageExceeded);

            pool.reserve_a = reserve_in
                .checked_add(amount_in_after_fee)
                .ok_or(HubError::MathOverflow)?;
            pool.reserve_b = reserve_out
                .checked_sub(amount_out)
                .ok_or(HubError::MathOverflow)?;
            (amount_out, fee, pool.token_a_mint, pool.token_b_mint)
        };

        let pool_seeds = &[
            b"liquidity_pool".as_ref(),
            token_a_mint.as_ref(),
            token_b_mint.as_ref(),
            &[ctx.bumps.liquidity_pool],
        ];
        let signer_seeds = &[&pool_seeds[..]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.pool_token_b_vault.to_account_info(),
                    to: ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.liquidity_pool.to_account_info(),
                },
                signer_seeds,
            ),
            amount_out,
        )?;

        emit!(SwapExecutedEvent {
            src_chain: msg.src_chain,
            sequence: msg.nonce,
            amount_in,
            amount_out,
            fee,
            recipient: msg.recipient,
        });
        Ok(())
    }

    /// Settle an attested swap as a plain transfer: the recipient takes the
    /// wrapped input asset instead of the swap output. Only the recipient can
    /// choose this, so a swap that keeps missing its slippage bound is never
    /// stuck.
    pub fn claim_swap_as_wrapped(
        ctx: Context<ClaimSwapAsWrapped>,
        payload_hash: [u8; 32],
        msg: InboundMessage,
    ) -> Result<()> {
        require!(
            canonical_hash(ACTION_SWAP, &msg) == payload_hash,
            HubError::PayloadHashMismatch
        );
        let record = &mut ctx.accounts.message_record;
        require!(!record.executed, HubError::MessageAlreadyExecuted);
        require!(record.action == ACTION_SWAP, HubError::WrongAction);
        record.executed = true;

        let amount = scale_amount(msg.amount, ctx.accounts.wrapped_asset.decimals_shift)?;
        let seeds = &[b"mint_authority".as_ref(), &[ctx.bumps.mint_authority]];
        let signer_seeds = &[&seeds[..]];
        token::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint: ctx.accounts.token_mint.to_account_info(),
                    to: ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.mint_authority.to_account_info(),
                },
                signer_seeds,
            ),
            amount,
        )?;

        emit!(MintEvent {
            src_chain: msg.src_chain,
            sequence: msg.nonce,
            recipient: msg.recipient,
            amount,
            payload_hash,
        });
        Ok(())
    }

    /// Stake ILINK tokens to become a relayer. Minimum 100,000 ILINK required.
    pub fn stake(ctx: Context<Stake>, amount: u64) -> Result<()> {
        require!(amount >= MIN_STAKE_AMOUNT, HubError::InsufficientStake);

        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.relayer_token_account.to_account_info(),
                    to: ctx.accounts.stake_vault.to_account_info(),
                    authority: ctx.accounts.relayer.to_account_info(),
                },
            ),
            amount,
        )?;

        let now = Clock::get()?.unix_timestamp;
        let stake_account = &mut ctx.accounts.stake_account;
        stake_account.relayer = ctx.accounts.relayer.key();
        stake_account.amount = stake_account
            .amount
            .checked_add(amount)
            .ok_or(HubError::MathOverflow)?;
        stake_account.staked_at = now;
        stake_account.last_active = now;

        let registry = &mut ctx.accounts.state_registry;
        registry.total_staked = registry
            .total_staked
            .checked_add(amount)
            .ok_or(HubError::MathOverflow)?;

        emit!(StakeEvent {
            relayer: ctx.accounts.relayer.key(),
            amount,
            total_staked: stake_account.amount,
        });
        Ok(())
    }

    /// Unstake ILINK tokens. Only allowed once `UNBONDING_PERIOD_SECS` have
    /// passed since the relayer last staked or attested a message.
    pub fn unstake(ctx: Context<Unstake>, amount: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let stake_account = &mut ctx.accounts.stake_account;
        let unlock_at = stake_account
            .last_active
            .checked_add(UNBONDING_PERIOD_SECS)
            .ok_or(HubError::MathOverflow)?;
        require!(now >= unlock_at, HubError::StakeLocked);
        require!(stake_account.amount >= amount, HubError::InsufficientStake);

        let remaining = stake_account.amount - amount;
        require!(
            remaining == 0 || remaining >= MIN_STAKE_AMOUNT,
            HubError::InsufficientStake
        );

        let relayer_key = stake_account.relayer;
        let vault_seeds = &[
            b"stake_vault".as_ref(),
            relayer_key.as_ref(),
            &[ctx.bumps.stake_vault],
        ];
        let signer_seeds = &[&vault_seeds[..]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.stake_vault.to_account_info(),
                    to: ctx.accounts.relayer_token_account.to_account_info(),
                    authority: ctx.accounts.stake_vault.to_account_info(),
                },
                signer_seeds,
            ),
            amount,
        )?;

        stake_account.amount = remaining;
        let registry = &mut ctx.accounts.state_registry;
        registry.total_staked = registry
            .total_staked
            .checked_sub(amount)
            .ok_or(HubError::MathOverflow)?;

        emit!(UnstakeEvent {
            relayer: ctx.accounts.relayer.key(),
            amount,
            remaining,
        });
        Ok(())
    }

    /// Slash a relayer. Burns 50% of their stake. Admin-only: there is no
    /// on-chain fraud proof yet, so slashing is a governance decision.
    pub fn slash_relayer(ctx: Context<SlashRelayer>) -> Result<()> {
        let stake_account = &mut ctx.accounts.stake_account;
        let slash_amount = mul_div(stake_account.amount, SLASH_BPS, 10_000)?;
        stake_account.amount -= slash_amount;

        let relayer_key = stake_account.relayer;
        let vault_seeds = &[
            b"stake_vault".as_ref(),
            relayer_key.as_ref(),
            &[ctx.bumps.stake_vault],
        ];
        let signer_seeds = &[&vault_seeds[..]];
        token::burn(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.ilink_mint.to_account_info(),
                    from: ctx.accounts.stake_vault.to_account_info(),
                    authority: ctx.accounts.stake_vault.to_account_info(),
                },
                signer_seeds,
            ),
            slash_amount,
        )?;

        let registry = &mut ctx.accounts.state_registry;
        registry.total_staked = registry
            .total_staked
            .checked_sub(slash_amount)
            .ok_or(HubError::MathOverflow)?;
        registry.total_burned = registry
            .total_burned
            .checked_add(slash_amount)
            .ok_or(HubError::MathOverflow)?;

        emit!(SlashEvent {
            relayer: relayer_key,
            slash_amount,
            remaining_stake: stake_account.amount,
        });
        Ok(())
    }

    /// Burn 40% of `amount` from the admin's fee vault. Admin-only, so random
    /// callers can no longer inflate the `total_burned` statistic.
    pub fn buy_back_and_burn(ctx: Context<BuyBackBurn>, amount: u64) -> Result<()> {
        let burn_amount = mul_div(amount, 40, 100)?;

        token::burn(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.ilink_mint.to_account_info(),
                    from: ctx.accounts.fee_vault.to_account_info(),
                    authority: ctx.accounts.admin.to_account_info(),
                },
            ),
            burn_amount,
        )?;

        let registry = &mut ctx.accounts.state_registry;
        registry.total_burned = registry
            .total_burned
            .checked_add(burn_amount)
            .ok_or(HubError::MathOverflow)?;

        emit!(TokenBurnedEvent {
            amount: burn_amount,
        });
        Ok(())
    }

    /// Initialize a liquidity pool. Admin-only; the vaults are program-owned
    /// PDAs created here, so nobody can squat a pool with vaults they control.
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        initial_reserve_a: u64,
        initial_reserve_b: u64,
    ) -> Result<()> {
        require!(
            initial_reserve_a > 0 && initial_reserve_b > 0,
            HubError::EmptyPool
        );

        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.admin_token_a.to_account_info(),
                    to: ctx.accounts.pool_token_a_vault.to_account_info(),
                    authority: ctx.accounts.admin.to_account_info(),
                },
            ),
            initial_reserve_a,
        )?;
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.admin_token_b.to_account_info(),
                    to: ctx.accounts.pool_token_b_vault.to_account_info(),
                    authority: ctx.accounts.admin.to_account_info(),
                },
            ),
            initial_reserve_b,
        )?;

        let pool = &mut ctx.accounts.liquidity_pool;
        pool.token_a_mint = ctx.accounts.token_a_mint.key();
        pool.token_b_mint = ctx.accounts.token_b_mint.key();
        pool.reserve_a = initial_reserve_a;
        pool.reserve_b = initial_reserve_b;
        pool.admin = ctx.accounts.admin.key();

        emit!(PoolInitializedEvent {
            token_a_mint: pool.token_a_mint,
            token_b_mint: pool.token_b_mint,
            reserve_a: initial_reserve_a,
            reserve_b: initial_reserve_b,
        });
        Ok(())
    }
}

// ─── Canonical message format ───────────────────────────────────────────────
//
// Every gateway hashes outbound messages the same way:
//
//   keccak256(abi.encode(
//       keccak256("InterlinkMessage(v1)"),
//       uint64  srcChain,   bytes32 srcGateway,
//       uint64  dstChain,   bytes32 dstGateway,
//       uint64  nonce,      uint8   action,
//       bytes32 sender,     bytes32 asset,
//       uint256 amount,     bytes32 recipient,
//       bytes32 extraHash))
//
// Binding both gateways and chain IDs stops replay across deployments.
// `extraHash` carries action-specific data (payload hash, swap params).

/// Fields of an inbound message, as emitted by the source gateway.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct InboundMessage {
    pub src_chain: u64,
    pub src_gateway: [u8; 32],
    pub nonce: u64,
    pub sender: [u8; 32],
    pub asset: [u8; 32],
    /// Source-chain amount (source decimals). The EVM gateway caps it at 2^128 − 1.
    pub amount: u128,
    pub recipient: Pubkey,
    pub extra_hash: [u8; 32],
}

fn word_u64(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

fn word_u128(v: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

/// All fields of a canonical message, in either direction.
pub struct MessageFields {
    pub src_chain: u64,
    pub src_gateway: [u8; 32],
    pub dst_chain: u64,
    pub dst_gateway: [u8; 32],
    pub nonce: u64,
    pub action: u8,
    pub sender: [u8; 32],
    pub asset: [u8; 32],
    pub amount: u128,
    pub recipient: [u8; 32],
    pub extra_hash: [u8; 32],
}

/// keccak256(abi.encode(typeHash, ...fields)), identical to `InterlinkGateway.hashMessage`.
pub fn message_hash(m: &MessageFields) -> [u8; 32] {
    let type_hash = keccak::hash(MESSAGE_TYPE_TAG).to_bytes();
    let mut action_word = [0u8; 32];
    action_word[31] = m.action;
    keccak::hashv(&[
        &type_hash,
        &word_u64(m.src_chain),
        &m.src_gateway,
        &word_u64(m.dst_chain),
        &m.dst_gateway,
        &word_u64(m.nonce),
        &action_word,
        &m.sender,
        &m.asset,
        &word_u128(m.amount),
        &m.recipient,
        &m.extra_hash,
    ])
    .to_bytes()
}

/// Canonical payload hash of an inbound message addressed to this hub.
pub fn canonical_hash(action: u8, m: &InboundMessage) -> [u8; 32] {
    message_hash(&MessageFields {
        src_chain: m.src_chain,
        src_gateway: m.src_gateway,
        dst_chain: HUB_CHAIN_ID,
        dst_gateway: crate::ID.to_bytes(),
        nonce: m.nonce,
        action,
        sender: m.sender,
        asset: m.asset,
        amount: m.amount,
        recipient: m.recipient.to_bytes(),
        extra_hash: m.extra_hash,
    })
}

/// `extraHash` for a swap: keccak256(abi.encode(bytes32 tokenOut, uint256 minAmountOut, bytes32 swapDataHash)).
pub fn swap_extra_hash(
    token_out: &Pubkey,
    min_amount_out: u64,
    swap_data_hash: &[u8; 32],
) -> [u8; 32] {
    keccak::hashv(&[
        &token_out.to_bytes(),
        &word_u64(min_amount_out),
        swap_data_hash,
    ])
    .to_bytes()
}

/// Convert a source-chain amount to local mint units. Dust below the
/// precision of the local mint stays locked on the source chain.
fn scale_amount(amount: u128, decimals_shift: u8) -> Result<u64> {
    let divisor = 10u128
        .checked_pow(decimals_shift as u32)
        .ok_or(HubError::InvalidDecimalsShift)?;
    let scaled = amount / divisor;
    require!(scaled > 0, HubError::AmountTooSmall);
    u64::try_from(scaled).map_err(|_| error!(HubError::AmountOverflow))
}

/// Inverse of `scale_amount`: local mint units back to source-chain units.
fn unscale_amount(amount: u64, decimals_shift: u8) -> Result<u128> {
    10u128
        .checked_pow(decimals_shift as u32)
        .and_then(|m| (amount as u128).checked_mul(m))
        .ok_or_else(|| error!(HubError::AmountOverflow))
}

fn mul_div(a: u64, b: u64, denom: u64) -> Result<u64> {
    let v = (a as u128)
        .checked_mul(b as u128)
        .ok_or(HubError::MathOverflow)?
        / denom as u128;
    u64::try_from(v).map_err(|_| error!(HubError::MathOverflow))
}

// ─── BN254 scalar field arithmetic ──────────────────────────────────────────
//
// 4×64-bit limb Montgomery arithmetic. The previous byte-wise
// shift-and-subtract reduction needed ~3,900 32-byte subtraction passes per
// multiplication, which puts the three multiplications in `verify` above
// Solana's 1.4M compute-unit limit. This is a few hundred operations each.

pub mod fr {
    /// r = 21888242871839275222246405745257275088548364400416034343698204186575808495617
    pub const MODULUS: [u64; 4] = [
        0x43e1_f593_f000_0001,
        0x2833_e848_79b9_7091,
        0xb850_45b6_8181_585d,
        0x3064_4e72_e131_a029,
    ];
    /// −r⁻¹ mod 2⁶⁴
    pub const INV: u64 = 0xc2e1_f593_efff_ffff;
    /// R² mod r, where R = 2²⁵⁶
    pub const R2: [u64; 4] = [
        0x1bb8_e645_ae21_6da7,
        0x53fe_3ab1_e35c_59e3,
        0x8c49_833d_53bb_8085,
        0x0216_d0b1_7f4e_44a5,
    ];

    pub type Fr = [u64; 4];

    #[inline(always)]
    fn mac(a: u64, b: u64, c: u64, carry: u64) -> (u64, u64) {
        let t = (a as u128) + (b as u128) * (c as u128) + (carry as u128);
        (t as u64, (t >> 64) as u64)
    }

    #[inline(always)]
    fn adc(a: u64, b: u64, carry: u64) -> (u64, u64) {
        let t = (a as u128) + (b as u128) + (carry as u128);
        (t as u64, (t >> 64) as u64)
    }

    fn geq_modulus(a: &Fr) -> bool {
        for i in (0..4).rev() {
            if a[i] != MODULUS[i] {
                return a[i] > MODULUS[i];
            }
        }
        true
    }

    fn sub_modulus(a: &Fr) -> Fr {
        let mut out = [0u64; 4];
        let mut borrow = 0u64;
        for i in 0..4 {
            let (d1, b1) = a[i].overflowing_sub(MODULUS[i]);
            let (d2, b2) = d1.overflowing_sub(borrow);
            out[i] = d2;
            borrow = (b1 | b2) as u64;
        }
        out
    }

    /// Montgomery product a·b·R⁻¹ mod r (CIOS).
    fn mont_mul(a: &Fr, b: &Fr) -> Fr {
        let mut t = [0u64; 6];
        for &bi in b.iter() {
            let mut carry = 0u64;
            for j in 0..4 {
                let (lo, hi) = mac(t[j], a[j], bi, carry);
                t[j] = lo;
                carry = hi;
            }
            let (s, c) = adc(t[4], carry, 0);
            t[4] = s;
            t[5] = c;

            let m = t[0].wrapping_mul(INV);
            let (_, mut carry) = mac(t[0], m, MODULUS[0], 0);
            for j in 1..4 {
                let (lo, hi) = mac(t[j], m, MODULUS[j], carry);
                t[j - 1] = lo;
                carry = hi;
            }
            let (s, c) = adc(t[4], carry, 0);
            t[3] = s;
            t[4] = t[5] + c;
        }
        let res = [t[0], t[1], t[2], t[3]];
        if t[4] != 0 || geq_modulus(&res) {
            sub_modulus(&res)
        } else {
            res
        }
    }

    /// a·b mod r for canonical inputs.
    pub fn mul(a: &Fr, b: &Fr) -> Fr {
        mont_mul(&mont_mul(a, b), &R2)
    }

    /// a+b mod r for canonical inputs (2r < 2²⁵⁶, so no overflow).
    pub fn add(a: &Fr, b: &Fr) -> Fr {
        let mut out = [0u64; 4];
        let mut carry = 0u64;
        for i in 0..4 {
            let (s, c) = adc(a[i], b[i], carry);
            out[i] = s;
            carry = c;
        }
        if geq_modulus(&out) {
            sub_modulus(&out)
        } else {
            out
        }
    }

    /// Interpret 32 big-endian bytes and reduce mod r. 2²⁵⁶ < 6r, so at most
    /// five subtractions (the old loop stopped after four).
    pub fn from_be_bytes_mod_order(bytes: &[u8; 32]) -> Fr {
        let mut x = [0u64; 4];
        for (i, limb) in x.iter_mut().enumerate() {
            let start = 32 - 8 * (i + 1);
            *limb = u64::from_be_bytes(bytes[start..start + 8].try_into().unwrap());
        }
        while geq_modulus(&x) {
            x = sub_modulus(&x);
        }
        x
    }

    pub fn from_u64(v: u64) -> Fr {
        [v, 0, 0, 0]
    }

    pub fn to_be_bytes(x: &Fr) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..4 {
            let start = 32 - 8 * (i + 1);
            out[start..start + 8].copy_from_slice(&x[i].to_be_bytes());
        }
        out
    }
}

/// Public input for the circuit: commitment = (msg + rc)^5 + sequence, where
/// msg = payload_hash mod r and rc = keccak256(DOMAIN_SALT) mod r.
/// Must match `relayer/src/prover.rs` exactly.
pub fn compute_commitment(payload_hash: &[u8; 32], sequence: u64) -> [u8; 32] {
    let rc = fr::from_be_bytes_mod_order(&keccak::hash(DOMAIN_SALT).to_bytes());
    let msg = fr::from_be_bytes_mod_order(payload_hash);
    let w = fr::add(&msg, &rc);
    let w2 = fr::mul(&w, &w);
    let w4 = fr::mul(&w2, &w2);
    let w5 = fr::mul(&w4, &w);
    fr::to_be_bytes(&fr::add(&w5, &fr::from_u64(sequence)))
}

// ─── Groth16 verification ───────────────────────────────────────────────────
//
//   e(−A, B) · e(α, β) · e(L, γ) · e(C, δ) = 1,   L = IC₀ + commitment · IC₁

/// BN254 base field modulus p (big-endian).
const BN254_BASE_FIELD: [u8; 32] = [
    0x30, 0x64, 0x4e, 0x72, 0xe1, 0x31, 0xa0, 0x29, 0xb8, 0x50, 0x45, 0xb6, 0x81, 0x81, 0x58, 0x5d,
    0x97, 0x81, 0x6a, 0x91, 0x68, 0x71, 0xca, 0x8d, 0x3c, 0x20, 0x8c, 0x16, 0xd8, 0x7c, 0xfd, 0x47,
];

/// Negate a G1 point: (x, y) → (x, p − y). The point at infinity (0, 0) is its own negation.
fn negate_g1(point: &[u8; 64]) -> [u8; 64] {
    let mut result = *point;
    if point[32..].iter().all(|b| *b == 0) {
        return result;
    }
    let mut borrow: u16 = 0;
    for i in (0..32).rev() {
        let diff = (BN254_BASE_FIELD[i] as u16)
            .wrapping_sub(point[32 + i] as u16)
            .wrapping_sub(borrow);
        result[32 + i] = diff as u8;
        borrow = if diff > 255 { 1 } else { 0 };
    }
    result
}

pub fn verify_groth16_proof(
    proof: &[u8],
    payload_hash: &[u8; 32],
    sequence: u64,
    vk: &VkBytes,
) -> bool {
    if proof.len() != 256 {
        return false;
    }
    let a: [u8; 64] = proof[0..64].try_into().unwrap();
    let b: [u8; 128] = proof[64..192].try_into().unwrap();
    let c: [u8; 64] = proof[192..256].try_into().unwrap();

    let commitment = compute_commitment(payload_hash, sequence);

    let mut ecmul_input = [0u8; 96];
    ecmul_input[0..64].copy_from_slice(&vk.ic_1);
    ecmul_input[64..96].copy_from_slice(&commitment);
    let scaled_ic1 = match alt_bn128_multiplication(&ecmul_input) {
        Ok(r) => r,
        Err(_) => return false,
    };

    let mut ecadd_input = [0u8; 128];
    ecadd_input[0..64].copy_from_slice(&vk.ic_0);
    ecadd_input[64..128].copy_from_slice(&scaled_ic1[..64]);
    let l_point = match alt_bn128_addition(&ecadd_input) {
        Ok(r) => r,
        Err(_) => return false,
    };

    let mut pairing_input = [0u8; 768];
    pairing_input[0..64].copy_from_slice(&negate_g1(&a));
    pairing_input[64..192].copy_from_slice(&b);
    pairing_input[192..256].copy_from_slice(&vk.alpha_g1);
    pairing_input[256..384].copy_from_slice(&vk.beta_g2);
    pairing_input[384..448].copy_from_slice(&l_point[..64]);
    pairing_input[448..576].copy_from_slice(&vk.gamma_g2);
    pairing_input[576..640].copy_from_slice(&c);
    pairing_input[640..768].copy_from_slice(&vk.delta_g2);

    match alt_bn128_pairing(&pairing_input) {
        Ok(result) => result[31] == 1 && result[..31].iter().all(|b| *b == 0),
        Err(_) => false,
    }
}

// ─── Account Contexts ───────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + StateRegistry::INIT_SPACE,
        seeds = [b"state"],
        bump
    )]
    pub state_registry: Account<'info, StateRegistry>,
    pub ilink_mint: Account<'info, Mint>,
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ HubError::Unauthorized)]
    pub program: Program<'info, crate::program::InterlinkHub>,
    #[account(constraint = program_data.upgrade_authority_address == Some(admin.key()) @ HubError::Unauthorized)]
    pub program_data: Account<'info, ProgramData>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetVerificationKey<'info> {
    #[account(mut, seeds = [b"state"], bump, has_one = admin)]
    pub state_registry: Account<'info, StateRegistry>,
    /// `init` (not `init_if_needed`): the initial key can be set exactly once.
    #[account(
        init,
        payer = admin,
        space = 8 + VerificationKey::INIT_SPACE,
        seeds = [b"vk"],
        bump
    )]
    pub verification_key: Account<'info, VerificationKey>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ProposeVerificationKey<'info> {
    #[account(seeds = [b"state"], bump, has_one = admin)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        init_if_needed,
        payer = admin,
        space = 8 + PendingVerificationKey::INIT_SPACE,
        seeds = [b"pending_vk"],
        bump
    )]
    pub pending_verification_key: Account<'info, PendingVerificationKey>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ApplyVerificationKey<'info> {
    #[account(seeds = [b"state"], bump, has_one = admin)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(mut, seeds = [b"pending_vk"], bump, close = admin)]
    pub pending_verification_key: Account<'info, PendingVerificationKey>,
    #[account(mut, seeds = [b"vk"], bump)]
    pub verification_key: Account<'info, VerificationKey>,
    #[account(mut)]
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(src_chain: u64, asset: [u8; 32])]
pub struct RegisterWrappedAsset<'info> {
    #[account(seeds = [b"state"], bump, has_one = admin)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        init,
        payer = admin,
        space = 8 + WrappedAsset::INIT_SPACE,
        seeds = [b"wrapped", src_chain.to_le_bytes().as_ref(), asset.as_ref()],
        bump
    )]
    pub wrapped_asset: Account<'info, WrappedAsset>,
    /// The hub must be the mint authority, otherwise transfers could never be executed.
    #[account(constraint = token_mint.mint_authority == anchor_lang::solana_program::program_option::COption::Some(mint_authority.key()) @ HubError::InvalidMint)]
    pub token_mint: Account<'info, Mint>,
    /// CHECK: PDA that signs mints; only its address is used.
    #[account(seeds = [b"mint_authority"], bump)]
    pub mint_authority: AccountInfo<'info>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(payload_hash: [u8; 32])]
pub struct SubmitProof<'info> {
    #[account(seeds = [b"state"], bump)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        mut,
        seeds = [b"stake", relayer.key().as_ref()],
        bump,
        has_one = relayer
    )]
    pub stake_account: Account<'info, StakeAccount>,
    #[account(seeds = [b"vk"], bump)]
    pub verification_key: Account<'info, VerificationKey>,
    /// Created here; `init` fails if this payload hash was already attested.
    #[account(
        init,
        payer = relayer,
        space = 8 + MessageRecord::INIT_SPACE,
        seeds = [b"message", payload_hash.as_ref()],
        bump
    )]
    pub message_record: Account<'info, MessageRecord>,
    #[account(mut)]
    pub relayer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(payload_hash: [u8; 32], msg: InboundMessage)]
pub struct ExecuteTransfer<'info> {
    /// Anyone may execute; pays for the recipient's token account if needed.
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut, seeds = [b"message", payload_hash.as_ref()], bump)]
    pub message_record: Account<'info, MessageRecord>,
    #[account(
        seeds = [b"wrapped", msg.src_chain.to_le_bytes().as_ref(), msg.asset.as_ref()],
        bump
    )]
    pub wrapped_asset: Account<'info, WrappedAsset>,
    #[account(mut, address = wrapped_asset.mint @ HubError::InvalidMint)]
    pub token_mint: Account<'info, Mint>,
    /// CHECK: only used as the token-account authority; bound to the hashed recipient.
    #[account(address = msg.recipient @ HubError::InvalidRecipient)]
    pub recipient: UncheckedAccount<'info>,
    /// The recipient's associated token account, created if missing so a
    /// transfer can never be stuck on a missing account.
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = token_mint,
        associated_token::authority = recipient
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,
    /// CHECK: PDA used as mint authority
    #[account(seeds = [b"mint_authority"], bump)]
    pub mint_authority: AccountInfo<'info>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(payload_hash: [u8; 32], msg: InboundMessage)]
pub struct ClaimSwapAsWrapped<'info> {
    /// Must be the swap's recipient.
    #[account(mut, address = msg.recipient @ HubError::InvalidRecipient)]
    pub recipient: Signer<'info>,
    #[account(mut, seeds = [b"message", payload_hash.as_ref()], bump)]
    pub message_record: Account<'info, MessageRecord>,
    #[account(
        seeds = [b"wrapped", msg.src_chain.to_le_bytes().as_ref(), msg.asset.as_ref()],
        bump
    )]
    pub wrapped_asset: Account<'info, WrappedAsset>,
    #[account(mut, address = wrapped_asset.mint @ HubError::InvalidMint)]
    pub token_mint: Account<'info, Mint>,
    #[account(
        init_if_needed,
        payer = recipient,
        associated_token::mint = token_mint,
        associated_token::authority = recipient
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,
    /// CHECK: PDA used as mint authority
    #[account(seeds = [b"mint_authority"], bump)]
    pub mint_authority: AccountInfo<'info>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(chain_id: u64)]
pub struct RegisterRemoteGateway<'info> {
    #[account(seeds = [b"state"], bump, has_one = admin)]
    pub state_registry: Account<'info, StateRegistry>,
    /// `init`: a chain's gateway can be set exactly once.
    #[account(
        init,
        payer = admin,
        space = 8 + RemoteGateway::INIT_SPACE,
        seeds = [b"remote", chain_id.to_le_bytes().as_ref()],
        bump
    )]
    pub remote_gateway: Account<'info, RemoteGateway>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct BurnForWithdrawal<'info> {
    #[account(mut, seeds = [b"state"], bump)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        seeds = [b"wrapped", wrapped_asset.src_chain.to_le_bytes().as_ref(), wrapped_asset.asset.as_ref()],
        bump
    )]
    pub wrapped_asset: Account<'info, WrappedAsset>,
    /// Collateral lives on the asset's origin chain, so that is the only destination.
    #[account(
        seeds = [b"remote", wrapped_asset.src_chain.to_le_bytes().as_ref()],
        bump
    )]
    pub remote_gateway: Account<'info, RemoteGateway>,
    #[account(mut, address = wrapped_asset.mint @ HubError::InvalidMint)]
    pub token_mint: Account<'info, Mint>,
    #[account(mut, token::mint = token_mint, token::authority = owner)]
    pub owner_token_account: Account<'info, TokenAccount>,
    pub owner: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(payload_hash: [u8; 32], msg: InboundMessage)]
pub struct ExecuteSwap<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [b"state"], bump)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(mut, seeds = [b"message", payload_hash.as_ref()], bump)]
    pub message_record: Account<'info, MessageRecord>,
    #[account(
        seeds = [b"wrapped", msg.src_chain.to_le_bytes().as_ref(), msg.asset.as_ref()],
        bump
    )]
    pub wrapped_asset: Account<'info, WrappedAsset>,
    #[account(
        mut,
        seeds = [
            b"liquidity_pool",
            liquidity_pool.token_a_mint.as_ref(),
            liquidity_pool.token_b_mint.as_ref(),
        ],
        bump,
        constraint = liquidity_pool.token_a_mint == wrapped_asset.mint @ HubError::WrongToken
    )]
    pub liquidity_pool: Account<'info, LiquidityPool>,
    #[account(
        mut,
        seeds = [b"pool_vault", liquidity_pool.key().as_ref(), liquidity_pool.token_b_mint.as_ref()],
        bump
    )]
    pub pool_token_b_vault: Account<'info, TokenAccount>,
    #[account(address = liquidity_pool.token_b_mint @ HubError::WrongToken)]
    pub token_b_mint: Account<'info, Mint>,
    /// CHECK: only used as the token-account authority; bound to the hashed recipient.
    #[account(address = msg.recipient @ HubError::InvalidRecipient)]
    pub recipient: UncheckedAccount<'info>,
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = token_b_mint,
        associated_token::authority = recipient
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Stake<'info> {
    #[account(mut, seeds = [b"state"], bump)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        init_if_needed,
        payer = relayer,
        space = 8 + StakeAccount::INIT_SPACE,
        seeds = [b"stake", relayer.key().as_ref()],
        bump
    )]
    pub stake_account: Account<'info, StakeAccount>,
    #[account(
        init_if_needed,
        payer = relayer,
        seeds = [b"stake_vault", relayer.key().as_ref()],
        bump,
        token::mint = ilink_mint,
        token::authority = stake_vault,
    )]
    pub stake_vault: Account<'info, TokenAccount>,
    /// Must be the real ILINK mint; otherwise a relayer could "stake" a worthless token.
    #[account(address = state_registry.ilink_mint @ HubError::InvalidMint)]
    pub ilink_mint: Account<'info, Mint>,
    #[account(mut)]
    pub relayer_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub relayer: Signer<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Unstake<'info> {
    #[account(mut, seeds = [b"state"], bump)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        mut,
        seeds = [b"stake", relayer.key().as_ref()],
        bump,
        has_one = relayer
    )]
    pub stake_account: Account<'info, StakeAccount>,
    #[account(mut, seeds = [b"stake_vault", relayer.key().as_ref()], bump)]
    pub stake_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub relayer_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub relayer: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct SlashRelayer<'info> {
    #[account(mut, seeds = [b"state"], bump, has_one = admin)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(mut, seeds = [b"stake", stake_account.relayer.as_ref()], bump)]
    pub stake_account: Account<'info, StakeAccount>,
    #[account(mut, seeds = [b"stake_vault", stake_account.relayer.as_ref()], bump)]
    pub stake_vault: Account<'info, TokenAccount>,
    #[account(mut, address = state_registry.ilink_mint @ HubError::InvalidMint)]
    pub ilink_mint: Account<'info, Mint>,
    pub admin: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct BuyBackBurn<'info> {
    #[account(mut, seeds = [b"state"], bump, has_one = admin)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(mut, address = state_registry.ilink_mint @ HubError::InvalidMint)]
    pub ilink_mint: Account<'info, Mint>,
    #[account(mut, token::mint = ilink_mint, token::authority = admin)]
    pub fee_vault: Account<'info, TokenAccount>,
    pub admin: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(seeds = [b"state"], bump, has_one = admin)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        init,
        payer = admin,
        space = 8 + LiquidityPool::INIT_SPACE,
        seeds = [
            b"liquidity_pool",
            token_a_mint.key().as_ref(),
            token_b_mint.key().as_ref(),
        ],
        bump
    )]
    pub liquidity_pool: Account<'info, LiquidityPool>,
    pub token_a_mint: Account<'info, Mint>,
    pub token_b_mint: Account<'info, Mint>,
    #[account(mut)]
    pub admin_token_a: Account<'info, TokenAccount>,
    #[account(mut)]
    pub admin_token_b: Account<'info, TokenAccount>,
    #[account(
        init,
        payer = admin,
        seeds = [b"pool_vault", liquidity_pool.key().as_ref(), token_a_mint.key().as_ref()],
        bump,
        token::mint = token_a_mint,
        token::authority = liquidity_pool,
    )]
    pub pool_token_a_vault: Account<'info, TokenAccount>,
    #[account(
        init,
        payer = admin,
        seeds = [b"pool_vault", liquidity_pool.key().as_ref(), token_b_mint.key().as_ref()],
        bump,
        token::mint = token_b_mint,
        token::authority = liquidity_pool,
    )]
    pub pool_token_b_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

// ─── State Accounts ─────────────────────────────────────────────────────────

#[account]
#[derive(InitSpace)]
pub struct StateRegistry {
    pub admin: Pubkey,
    pub ilink_mint: Pubkey,
    pub fee_rate_bps: u16,
    pub total_staked: u64,
    pub total_burned: u64,
    pub vk_initialized: bool,
    /// Next nonce for messages leaving the hub (`burn_for_withdrawal`).
    pub outbound_nonce: u64,
}

/// Trusted gateway on a remote chain (destination of withdrawals).
#[account]
#[derive(InitSpace)]
pub struct RemoteGateway {
    pub chain_id: u64,
    pub gateway: [u8; 32],
}

/// Raw Groth16 verification key (EVM/Solana precompile encoding).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, InitSpace)]
pub struct VkBytes {
    pub alpha_g1: [u8; 64],
    pub beta_g2: [u8; 128],
    pub gamma_g2: [u8; 128],
    pub delta_g2: [u8; 128],
    pub ic_0: [u8; 64],
    pub ic_1: [u8; 64],
}

impl VkBytes {
    pub fn try_from_slice_checked(data: &[u8]) -> Result<Self> {
        require!(data.len() == 576, HubError::InvalidVKLength);
        Ok(Self {
            alpha_g1: data[0..64].try_into().unwrap(),
            beta_g2: data[64..192].try_into().unwrap(),
            gamma_g2: data[192..320].try_into().unwrap(),
            delta_g2: data[320..448].try_into().unwrap(),
            ic_0: data[448..512].try_into().unwrap(),
            ic_1: data[512..576].try_into().unwrap(),
        })
    }
}

#[account]
#[derive(InitSpace)]
pub struct VerificationKey {
    pub key: VkBytes,
}

#[account]
#[derive(InitSpace)]
pub struct PendingVerificationKey {
    pub key: VkBytes,
    pub eta: i64,
}

#[account]
#[derive(InitSpace)]
pub struct MessageRecord {
    pub payload_hash: [u8; 32],
    pub action: u8,
    pub executed: bool,
    pub attested_by: Pubkey,
    pub attested_at: i64,
}

#[account]
#[derive(InitSpace)]
pub struct WrappedAsset {
    pub src_chain: u64,
    pub asset: [u8; 32],
    pub mint: Pubkey,
    pub decimals_shift: u8,
}

#[account]
#[derive(InitSpace)]
pub struct StakeAccount {
    pub relayer: Pubkey,
    pub amount: u64,
    pub staked_at: i64,
    /// Last stake or attestation; unbonding is measured from here.
    pub last_active: i64,
}

#[account]
#[derive(InitSpace)]
pub struct LiquidityPool {
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub admin: Pubkey,
}

// ─── Events ─────────────────────────────────────────────────────────────────

#[event]
pub struct ProofVerifiedEvent {
    pub src_chain: u64,
    pub sequence: u64,
    pub payload_hash: [u8; 32],
    pub relayer: Pubkey,
}

#[event]
pub struct MintEvent {
    pub src_chain: u64,
    pub sequence: u64,
    pub recipient: Pubkey,
    pub amount: u64,
    pub payload_hash: [u8; 32],
}

#[event]
pub struct SwapExecutedEvent {
    pub src_chain: u64,
    pub sequence: u64,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee: u64,
    pub recipient: Pubkey,
}

#[event]
pub struct StakeEvent {
    pub relayer: Pubkey,
    pub amount: u64,
    pub total_staked: u64,
}

#[event]
pub struct UnstakeEvent {
    pub relayer: Pubkey,
    pub amount: u64,
    pub remaining: u64,
}

#[event]
pub struct SlashEvent {
    pub relayer: Pubkey,
    pub slash_amount: u64,
    pub remaining_stake: u64,
}

#[event]
pub struct TokenBurnedEvent {
    pub amount: u64,
}

#[event]
pub struct PoolInitializedEvent {
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub reserve_a: u64,
    pub reserve_b: u64,
}

#[event]
pub struct VKUpdatedEvent {
    pub admin: Pubkey,
}

#[event]
pub struct VKProposedEvent {
    pub admin: Pubkey,
    pub eta: i64,
}

/// A message leaving the hub. Source chain is `HUB_CHAIN_ID` and source
/// gateway is this program's ID; relayers prove `payload_hash` and deliver it
/// to the destination gateway (`releaseTokens` on EVM).
#[event]
pub struct OutboundMessageEvent {
    pub payload_hash: [u8; 32],
    pub dst_chain: u64,
    pub dst_gateway: [u8; 32],
    pub nonce: u64,
    pub action: u8,
    pub sender: [u8; 32],
    pub asset: [u8; 32],
    pub amount: u128,
    pub recipient: [u8; 32],
    pub extra_hash: [u8; 32],
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[error_code]
pub enum HubError {
    #[msg("Invalid ZK proof: Groth16 pairing check failed.")]
    InvalidProof,
    #[msg("Proof must be exactly 256 bytes.")]
    InvalidProofLength,
    #[msg("Insufficient stake: minimum 100,000 ILINK required.")]
    InsufficientStake,
    #[msg("Slippage exceeded: output amount below minimum.")]
    SlippageExceeded,
    #[msg("Liquidity pool is empty.")]
    EmptyPool,
    #[msg("Verification key not initialized. Call set_verification_key first.")]
    VKNotInitialized,
    #[msg("Verification key data must be exactly 576 bytes.")]
    InvalidVKLength,
    #[msg("Signer is not authorized for this action.")]
    Unauthorized,
    #[msg("Message fields do not hash to the supplied payload hash.")]
    PayloadHashMismatch,
    #[msg("Message has already been executed.")]
    MessageAlreadyExecuted,
    #[msg("Message action does not match this instruction.")]
    WrongAction,
    #[msg("Fee rate exceeds the maximum allowed.")]
    InvalidFeeRate,
    #[msg("Stake is still within its unbonding period.")]
    StakeLocked,
    #[msg("Verification key timelock has not elapsed.")]
    TimelockNotElapsed,
    #[msg("Amount does not fit in a u64 after decimal scaling.")]
    AmountOverflow,
    #[msg("Amount rounds to zero after decimal scaling.")]
    AmountTooSmall,
    #[msg("Arithmetic overflow.")]
    MathOverflow,
    #[msg("Recipient token account is not owned by the message recipient.")]
    InvalidRecipient,
    #[msg("Mint does not match the registered asset.")]
    InvalidMint,
    #[msg("Token does not match the pool or message.")]
    WrongToken,
    #[msg("Decimals shift is out of range.")]
    InvalidDecimalsShift,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ark_bn254::{Bn254, Fr as ArkFr, G1Affine, G2Affine};
    use ark_ff::{BigInteger, Field, PrimeField, UniformRand};
    use ark_groth16::Groth16;
    use ark_relations::r1cs::{
        ConstraintSynthesizer, ConstraintSystemRef, LinearCombination, SynthesisError, Variable,
    };
    use ark_snark::SNARK;
    use ark_std::rand::{rngs::StdRng, SeedableRng};

    fn ark_to_limbs(x: ArkFr) -> fr::Fr {
        x.into_bigint().0
    }

    fn limbs_to_ark(x: &fr::Fr) -> ArkFr {
        ArkFr::from_be_bytes_mod_order(&fr::to_be_bytes(x))
    }

    #[test]
    fn fr_constants_are_correct() {
        assert_eq!(fr::MODULUS, ArkFr::MODULUS.0);
        // r · INV ≡ −1 (mod 2^64)
        assert_eq!(fr::MODULUS[0].wrapping_mul(fr::INV), u64::MAX);
        // R² mod r, R = 2^256
        assert_eq!(fr::R2, ark_to_limbs(ArkFr::from(2u64).pow([512u64])));
    }

    #[test]
    fn fr_mul_and_add_match_arkworks() {
        let mut rng = StdRng::seed_from_u64(7);
        for _ in 0..2_000 {
            let a = ArkFr::rand(&mut rng);
            let b = ArkFr::rand(&mut rng);
            let (la, lb) = (ark_to_limbs(a), ark_to_limbs(b));
            assert_eq!(limbs_to_ark(&fr::mul(&la, &lb)), a * b);
            assert_eq!(limbs_to_ark(&fr::add(&la, &lb)), a + b);
        }
        let max = ark_to_limbs(-ArkFr::from(1u64));
        assert_eq!(limbs_to_ark(&fr::mul(&max, &max)), ArkFr::from(1u64));
    }

    #[test]
    fn reduction_handles_values_above_5r() {
        // 0xFF..FF ≥ 5r needs five subtractions; the old loop stopped at four.
        for bytes in [[0xFFu8; 32], [0xF3u8; 32], [0u8; 32]] {
            let ours = limbs_to_ark(&fr::from_be_bytes_mod_order(&bytes));
            assert_eq!(ours, ArkFr::from_be_bytes_mod_order(&bytes));
        }
    }

    fn reference_commitment(payload_hash: &[u8; 32], sequence: u64) -> ArkFr {
        let rc = ArkFr::from_be_bytes_mod_order(&keccak::hash(DOMAIN_SALT).to_bytes());
        let w = ArkFr::from_be_bytes_mod_order(payload_hash) + rc;
        w.square().square() * w + ArkFr::from(sequence)
    }

    #[test]
    fn commitment_matches_relayer_formula() {
        let mut rng = StdRng::seed_from_u64(11);
        for i in 0..200u64 {
            let mut h = [0u8; 32];
            for b in h.iter_mut() {
                *b = (u64::rand(&mut rng) & 0xFF) as u8;
            }
            if i % 10 == 0 {
                h = [0xFF; 32];
            }
            let ours = compute_commitment(&h, i * 1_000_003);
            let expected = reference_commitment(&h, i * 1_000_003)
                .into_bigint()
                .to_bytes_be();
            assert_eq!(ours.to_vec(), expected);
        }
    }

    fn sample_message() -> InboundMessage {
        InboundMessage {
            src_chain: 1,
            src_gateway: [0x11; 32],
            nonce: 42,
            sender: [0x22; 32],
            asset: [0x33; 32],
            amount: 1_000_000_000_000_000_000,
            recipient: Pubkey::new_from_array([0x44; 32]),
            extra_hash: [0x55; 32],
        }
    }

    #[test]
    fn canonical_hash_matches_abi_encoding() {
        // Independent encoding: twelve 32-byte ABI words, as abi.encode produces.
        let m = sample_message();
        let mut words: Vec<[u8; 32]> = Vec::new();
        words.push(keccak::hash(b"InterlinkMessage(v1)").to_bytes());
        let mut w = [0u8; 32];
        w[31] = 1;
        words.push(w); // src_chain = 1
        words.push([0x11; 32]);
        let mut w = [0u8; 32];
        w[31] = 2;
        words.push(w); // dst_chain = HUB_CHAIN_ID
        words.push(crate::ID.to_bytes());
        let mut w = [0u8; 32];
        w[31] = 42;
        words.push(w); // nonce
        let mut w = [0u8; 32];
        w[31] = ACTION_TRANSFER;
        words.push(w);
        words.push([0x22; 32]);
        words.push([0x33; 32]);
        let mut w = [0u8; 32];
        w[16..].copy_from_slice(&1_000_000_000_000_000_000u128.to_be_bytes());
        words.push(w);
        words.push([0x44; 32]);
        words.push([0x55; 32]);
        let flat: Vec<u8> = words.concat();
        assert_eq!(
            canonical_hash(ACTION_TRANSFER, &m),
            keccak::hash(&flat).to_bytes()
        );
    }

    #[test]
    fn canonical_hash_binds_every_field() {
        let base = sample_message();
        let h = canonical_hash(ACTION_TRANSFER, &base);
        let mutations: Vec<Box<dyn Fn(&mut InboundMessage)>> = vec![
            Box::new(|m| m.src_chain += 1),
            Box::new(|m| m.src_gateway[0] ^= 1),
            Box::new(|m| m.nonce += 1),
            Box::new(|m| m.sender[0] ^= 1),
            Box::new(|m| m.asset[0] ^= 1),
            Box::new(|m| m.amount += 1),
            Box::new(|m| m.recipient = Pubkey::new_from_array([0x45; 32])),
            Box::new(|m| m.extra_hash[0] ^= 1),
        ];
        for mutate in mutations {
            let mut m = base.clone();
            mutate(&mut m);
            assert_ne!(canonical_hash(ACTION_TRANSFER, &m), h);
        }
        assert_ne!(canonical_hash(ACTION_SWAP, &base), h);
    }

    #[test]
    fn outbound_hash_matches_evm_vector() {
        // Same vector as contracts/evm/test (VECTOR_HASH), checked against the
        // compiled gateway: a hub → EVM transfer that `releaseTokens` accepts.
        let mut dst_gateway = [0u8; 32];
        dst_gateway[30..].copy_from_slice(&[0xBE, 0xEF]);
        let mut recipient = [0u8; 32];
        recipient[12..].copy_from_slice(&[0xA1; 20]);
        let h = message_hash(&MessageFields {
            src_chain: HUB_CHAIN_ID,
            src_gateway: [0x7B; 32],
            dst_chain: 31337,
            dst_gateway,
            nonce: 7,
            action: ACTION_TRANSFER,
            sender: [0x22; 32],
            asset: [0; 32],
            amount: 2_000_000_000_000_000_000,
            recipient,
            extra_hash: [0; 32],
        });
        let expected: [u8; 32] = [
            0x07, 0xb0, 0xf8, 0xb8, 0xce, 0x5c, 0x6f, 0x82, 0xe0, 0xbb, 0x55, 0x20, 0x73, 0xf1,
            0xc3, 0x5d, 0xf9, 0xb0, 0x98, 0x7c, 0x6e, 0x2c, 0x59, 0x8c, 0x52, 0xed, 0x5b, 0x59,
            0xee, 0x1c, 0x09, 0xbd,
        ];
        assert_eq!(h, expected);
    }

    #[test]
    fn unscale_round_trips_scale() {
        // 1.23456789 ETH bridged to an 8-decimal mint and back loses only dust.
        let src = 1_234_567_891_234_567_891u128;
        let local = scale_amount(src, 10).unwrap();
        assert_eq!(
            unscale_amount(local, 10).unwrap(),
            1_234_567_890_000_000_000
        );
        assert!(unscale_amount(u64::MAX, 30).is_err());
    }

    #[test]
    fn scale_amount_handles_decimals() {
        // 1 ETH (18 decimals) → 8-decimal mint
        assert_eq!(
            scale_amount(1_000_000_000_000_000_000, 10).unwrap(),
            100_000_000
        );
        // 100 ETH at 18 decimals overflows u64 without scaling
        assert!(scale_amount(100_000_000_000_000_000_000, 0).is_err());
        assert!(scale_amount(5, 1).is_err()); // rounds to zero
    }

    // Same circuit as relayer/src/prover.rs.
    #[derive(Clone)]
    struct InterlinkCircuit {
        message: Option<ArkFr>,
        sequence: Option<ArkFr>,
        round_constant: ArkFr,
    }

    impl ConstraintSynthesizer<ArkFr> for InterlinkCircuit {
        fn generate_constraints(
            self,
            cs: ConstraintSystemRef<ArkFr>,
        ) -> std::result::Result<(), SynthesisError> {
            let rc = self.round_constant;
            let msg =
                cs.new_witness_variable(|| self.message.ok_or(SynthesisError::AssignmentMissing))?;
            let seq =
                cs.new_witness_variable(|| self.sequence.ok_or(SynthesisError::AssignmentMissing))?;
            let w_val = self.message.map(|m| m + rc);
            let w = cs.new_witness_variable(|| w_val.ok_or(SynthesisError::AssignmentMissing))?;
            let mut a = LinearCombination::zero();
            a += (ArkFr::from(1u64), msg);
            a += (rc, Variable::One);
            cs.enforce_constraint(
                a,
                LinearCombination::from(Variable::One),
                LinearCombination::from(w),
            )?;
            let v1_val = w_val.map(|w| w.square());
            let v1 = cs.new_witness_variable(|| v1_val.ok_or(SynthesisError::AssignmentMissing))?;
            cs.enforce_constraint(
                LinearCombination::from(w),
                LinearCombination::from(w),
                LinearCombination::from(v1),
            )?;
            let v2_val = v1_val.map(|v| v.square());
            let v2 = cs.new_witness_variable(|| v2_val.ok_or(SynthesisError::AssignmentMissing))?;
            cs.enforce_constraint(
                LinearCombination::from(v1),
                LinearCombination::from(v1),
                LinearCombination::from(v2),
            )?;
            let v3_val = v2_val.zip(w_val).map(|(v2, w)| v2 * w);
            let v3 = cs.new_witness_variable(|| v3_val.ok_or(SynthesisError::AssignmentMissing))?;
            cs.enforce_constraint(
                LinearCombination::from(v2),
                LinearCombination::from(w),
                LinearCombination::from(v3),
            )?;
            let c_val = v3_val.zip(self.sequence).map(|(v3, s)| v3 + s);
            let c = cs.new_input_variable(|| c_val.ok_or(SynthesisError::AssignmentMissing))?;
            let mut a = LinearCombination::zero();
            a += (ArkFr::from(1u64), v3);
            a += (ArkFr::from(1u64), seq);
            cs.enforce_constraint(
                a,
                LinearCombination::from(Variable::One),
                LinearCombination::from(c),
            )?;
            Ok(())
        }
    }

    fn g1_bytes(p: &G1Affine) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(&p.x.into_bigint().to_bytes_be());
        out[32..].copy_from_slice(&p.y.into_bigint().to_bytes_be());
        out
    }

    fn g2_bytes(p: &G2Affine) -> [u8; 128] {
        let mut out = [0u8; 128];
        out[..32].copy_from_slice(&p.x.c1.into_bigint().to_bytes_be());
        out[32..64].copy_from_slice(&p.x.c0.into_bigint().to_bytes_be());
        out[64..96].copy_from_slice(&p.y.c1.into_bigint().to_bytes_be());
        out[96..].copy_from_slice(&p.y.c0.into_bigint().to_bytes_be());
        out
    }

    /// End-to-end: a real arkworks Groth16 proof verifies through the
    /// on-chain verifier (host alt_bn128 implementation), and any change to
    /// the message or sequence is rejected.
    #[test]
    fn groth16_end_to_end() {
        let mut rng = StdRng::seed_from_u64(3);
        let rc = ArkFr::from_be_bytes_mod_order(&keccak::hash(DOMAIN_SALT).to_bytes());
        let blank = InterlinkCircuit {
            message: None,
            sequence: None,
            round_constant: rc,
        };
        let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(blank, &mut rng).unwrap();

        let msg = sample_message();
        let payload_hash = canonical_hash(ACTION_TRANSFER, &msg);
        let circuit = InterlinkCircuit {
            message: Some(ArkFr::from_be_bytes_mod_order(&payload_hash)),
            sequence: Some(ArkFr::from(msg.nonce)),
            round_constant: rc,
        };
        let proof = Groth16::<Bn254>::prove(&pk, circuit, &mut rng).unwrap();

        let mut proof_bytes = Vec::with_capacity(256);
        proof_bytes.extend_from_slice(&g1_bytes(&proof.a));
        proof_bytes.extend_from_slice(&g2_bytes(&proof.b));
        proof_bytes.extend_from_slice(&g1_bytes(&proof.c));

        let vk_bytes = VkBytes {
            alpha_g1: g1_bytes(&vk.alpha_g1),
            beta_g2: g2_bytes(&vk.beta_g2),
            gamma_g2: g2_bytes(&vk.gamma_g2),
            delta_g2: g2_bytes(&vk.delta_g2),
            ic_0: g1_bytes(&vk.gamma_abc_g1[0]),
            ic_1: g1_bytes(&vk.gamma_abc_g1[1]),
        };

        assert!(verify_groth16_proof(
            &proof_bytes,
            &payload_hash,
            msg.nonce,
            &vk_bytes
        ));

        let mut tampered = msg.clone();
        tampered.amount *= 1_000;
        let bad_hash = canonical_hash(ACTION_TRANSFER, &tampered);
        assert!(!verify_groth16_proof(
            &proof_bytes,
            &bad_hash,
            msg.nonce,
            &vk_bytes
        ));
        assert!(!verify_groth16_proof(
            &proof_bytes,
            &payload_hash,
            msg.nonce + 1,
            &vk_bytes
        ));

        let mut corrupted = proof_bytes.clone();
        corrupted[200] ^= 1;
        assert!(!verify_groth16_proof(
            &corrupted,
            &payload_hash,
            msg.nonce,
            &vk_bytes
        ));
    }
}
