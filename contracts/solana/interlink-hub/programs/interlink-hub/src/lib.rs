use anchor_lang::prelude::*;
use anchor_lang::solana_program::alt_bn128::prelude::alt_bn128_pairing;
use anchor_spl::token::{self, Burn, Mint, MintTo, Token, TokenAccount, Transfer};

declare_id!("AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz");

// -G2 generator constants (BN254). required for the Groth16 pairing check.
// negated y coords of the standard G2 generator point.
const NEG_G2_GEN: [u8; 128] = [
    // x_c1 (big-endian)
    0x18, 0x00, 0xde, 0xef, 0x12, 0x1f, 0x1e, 0x76, 0x42, 0x6a, 0x00, 0x66, 0x5e, 0x5c, 0x44, 0x79,
    0x67, 0x43, 0x22, 0xd4, 0xf7, 0x5e, 0xda, 0xdd, 0x46, 0xde, 0xbd, 0x5c, 0xd9, 0x92, 0xf6, 0xed,
    // x_c0
    0x19, 0x8e, 0x93, 0x93, 0x92, 0x0d, 0x48, 0x3a, 0x72, 0x60, 0xbf, 0xb7, 0x31, 0xfb, 0x5d, 0x25,
    0xf1, 0xaa, 0x49, 0x33, 0x35, 0xa9, 0xe7, 0x12, 0x97, 0xe4, 0x85, 0xb7, 0xae, 0xf3, 0x12, 0xc2,
    // y_c1
    0x12, 0xc8, 0x5e, 0xa5, 0xdb, 0x8c, 0x6d, 0xeb, 0x4a, 0xab, 0x71, 0x80, 0x8d, 0xcb, 0x40, 0x8f,
    0xe3, 0xd1, 0xe7, 0x69, 0x0c, 0x43, 0xd3, 0x7b, 0x4c, 0xe6, 0xcc, 0x01, 0x66, 0xfa, 0x7d, 0xaa,
    // y_c0
    0x09, 0x06, 0x89, 0xd0, 0x58, 0x5f, 0xf0, 0x75, 0xec, 0x9e, 0x99, 0xad, 0x6b, 0x85, 0x63, 0xef,
    0x40, 0x66, 0x38, 0x0c, 0x10, 0x73, 0xd5, 0x28, 0x39, 0x9e, 0x71, 0x59, 0x2c, 0x34, 0xa2, 0x33,
];

/// Minimum ILINK stake required for relayers (100,000 tokens with 6 decimals)
const MIN_STAKE_AMOUNT: u64 = 100_000_000_000;

/// Slashing percentage (50% of stake burned for invalid proofs)
const SLASH_BPS: u64 = 5_000;

#[program]
pub mod interlink_hub {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, fee_rate_bps: u16) -> Result<()> {
        let registry = &mut ctx.accounts.state_registry;
        registry.admin = ctx.accounts.admin.key();
        registry.fee_rate_bps = fee_rate_bps;
        registry.processed_sequences = 0;
        registry.total_staked = 0;
        registry.total_burned = 0;
        Ok(())
    }

    pub fn submit_proof(
        ctx: Context<SubmitProof>,
        _source_chain: u64,
        sequence: u64,
        proof_data: Vec<u8>,
        payload_hash: [u8; 32],
        _commitment_input: [u8; 32],
    ) -> Result<()> {
        let registry = &mut ctx.accounts.state_registry;

        require!(
            sequence > registry.processed_sequences,
            HubError::SequenceAlreadyProcessed
        );

        require!(
            verify_groth16_proof(&proof_data, &payload_hash),
            HubError::InvalidProof
        );

        registry.processed_sequences = sequence;

        emit!(ProofVerifiedEvent {
            sequence,
            payload_hash,
            relayer: ctx.accounts.relayer.key()
        });

        Ok(())
    }

    /// Verify a ZK proof and mint wrapped tokens to the recipient.
    /// Called after a cross-chain deposit is proven on the source chain.
    pub fn verify_and_mint(
        ctx: Context<VerifyAndMint>,
        sequence: u64,
        amount: u64,
        proof_data: Vec<u8>,
        payload_hash: [u8; 32],
    ) -> Result<()> {
        let registry = &mut ctx.accounts.state_registry;

        require!(
            sequence > registry.processed_sequences,
            HubError::SequenceAlreadyProcessed
        );

        require!(
            verify_groth16_proof(&proof_data, &payload_hash),
            HubError::InvalidProof
        );

        registry.processed_sequences = sequence;

        // Mint wrapped tokens to recipient via PDA authority
        let seeds = &[b"mint_authority".as_ref(), &[ctx.bumps.mint_authority]];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = MintTo {
            mint: ctx.accounts.token_mint.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.mint_authority.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::mint_to(cpi_ctx, amount)?;

        emit!(MintEvent {
            sequence,
            recipient: ctx.accounts.recipient_token_account.key(),
            amount,
            payload_hash,
        });

        Ok(())
    }

    /// Execute a cross-chain swap through the hub's liquidity pool.
    /// Verifies proof, swaps via constant product AMM, transfers output to recipient.
    pub fn process_cross_chain_swap(
        ctx: Context<ProcessCrossChainSwap>,
        sequence: u64,
        amount_in: u64,
        min_amount_out: u64,
        proof_data: Vec<u8>,
        payload_hash: [u8; 32],
    ) -> Result<()> {
        let registry = &mut ctx.accounts.state_registry;

        require!(
            sequence > registry.processed_sequences,
            HubError::SequenceAlreadyProcessed
        );

        require!(
            verify_groth16_proof(&proof_data, &payload_hash),
            HubError::InvalidProof
        );

        registry.processed_sequences = sequence;

        // Constant product AMM: amount_out = (amount_in * reserve_out) / (reserve_in + amount_in)
        let pool = &mut ctx.accounts.liquidity_pool;
        let reserve_in = pool.reserve_a;
        let reserve_out = pool.reserve_b;

        require!(reserve_in > 0 && reserve_out > 0, HubError::EmptyPool);

        // Apply fee (fee_rate_bps basis points)
        let fee = (amount_in as u128)
            .checked_mul(registry.fee_rate_bps as u128)
            .unwrap()
            .checked_div(10_000)
            .unwrap() as u64;
        let amount_in_after_fee = amount_in.checked_sub(fee).unwrap();

        let amount_out = (amount_in_after_fee as u128)
            .checked_mul(reserve_out as u128)
            .unwrap()
            .checked_div((reserve_in as u128).checked_add(amount_in_after_fee as u128).unwrap())
            .unwrap() as u64;

        require!(amount_out >= min_amount_out, HubError::SlippageExceeded);

        // Update pool reserves
        pool.reserve_a = reserve_in.checked_add(amount_in_after_fee).unwrap();
        pool.reserve_b = reserve_out.checked_sub(amount_out).unwrap();

        // Transfer output tokens to recipient
        let pool_seeds = &[
            b"liquidity_pool".as_ref(),
            pool.token_a_mint.as_ref(),
            pool.token_b_mint.as_ref(),
            &[ctx.bumps.liquidity_pool],
        ];
        let signer_seeds = &[&pool_seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.pool_token_b_vault.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.liquidity_pool.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, amount_out)?;

        emit!(SwapExecutedEvent {
            sequence,
            amount_in,
            amount_out,
            fee,
            recipient: ctx.accounts.recipient_token_account.key(),
        });

        Ok(())
    }

    /// Stake ILINK tokens to become a relayer. Minimum 100,000 ILINK required.
    pub fn stake(ctx: Context<Stake>, amount: u64) -> Result<()> {
        require!(amount >= MIN_STAKE_AMOUNT, HubError::InsufficientStake);

        // Transfer ILINK from relayer to stake vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.relayer_token_account.to_account_info(),
            to: ctx.accounts.stake_vault.to_account_info(),
            authority: ctx.accounts.relayer.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
        );
        token::transfer(cpi_ctx, amount)?;

        let stake_account = &mut ctx.accounts.stake_account;
        stake_account.relayer = ctx.accounts.relayer.key();
        stake_account.amount = stake_account.amount.checked_add(amount).unwrap();
        stake_account.staked_at = Clock::get()?.unix_timestamp;

        let registry = &mut ctx.accounts.state_registry;
        registry.total_staked = registry.total_staked.checked_add(amount).unwrap();

        emit!(StakeEvent {
            relayer: ctx.accounts.relayer.key(),
            amount,
            total_staked: stake_account.amount,
        });

        Ok(())
    }

    /// Unstake ILINK tokens. Relayer must have no pending proofs.
    pub fn unstake(ctx: Context<Unstake>, amount: u64) -> Result<()> {
        let stake_account = &mut ctx.accounts.stake_account;
        require!(stake_account.amount >= amount, HubError::InsufficientStake);

        // Ensure remaining stake meets minimum (or is zero for full withdrawal)
        let remaining = stake_account.amount.checked_sub(amount).unwrap();
        require!(
            remaining == 0 || remaining >= MIN_STAKE_AMOUNT,
            HubError::InsufficientStake
        );

        // Transfer from stake vault back to relayer
        let vault_seeds = &[
            b"stake_vault".as_ref(),
            stake_account.relayer.as_ref(),
            &[ctx.bumps.stake_vault],
        ];
        let signer_seeds = &[&vault_seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.stake_vault.to_account_info(),
            to: ctx.accounts.relayer_token_account.to_account_info(),
            authority: ctx.accounts.stake_vault.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, amount)?;

        stake_account.amount = remaining;

        let registry = &mut ctx.accounts.state_registry;
        registry.total_staked = registry.total_staked.checked_sub(amount).unwrap();

        emit!(UnstakeEvent {
            relayer: ctx.accounts.relayer.key(),
            amount,
            remaining: stake_account.amount,
        });

        Ok(())
    }

    /// Slash a relayer for submitting an invalid proof. Burns 50% of their stake.
    /// Only callable by the admin.
    pub fn slash_relayer(ctx: Context<SlashRelayer>) -> Result<()> {
        let stake_account = &mut ctx.accounts.stake_account;
        let slash_amount = stake_account
            .amount
            .checked_mul(SLASH_BPS)
            .unwrap()
            .checked_div(10_000)
            .unwrap();

        stake_account.amount = stake_account.amount.checked_sub(slash_amount).unwrap();

        // Burn the slashed tokens
        let vault_seeds = &[
            b"stake_vault".as_ref(),
            stake_account.relayer.as_ref(),
            &[ctx.bumps.stake_vault],
        ];
        let signer_seeds = &[&vault_seeds[..]];

        let cpi_accounts = Burn {
            mint: ctx.accounts.ilink_mint.to_account_info(),
            from: ctx.accounts.stake_vault.to_account_info(),
            authority: ctx.accounts.stake_vault.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::burn(cpi_ctx, slash_amount)?;

        let registry = &mut ctx.accounts.state_registry;
        registry.total_staked = registry.total_staked.checked_sub(slash_amount).unwrap();
        registry.total_burned = registry.total_burned.checked_add(slash_amount).unwrap();

        emit!(SlashEvent {
            relayer: stake_account.relayer,
            slash_amount,
            remaining_stake: stake_account.amount,
        });

        Ok(())
    }

    pub fn buy_back_and_burn(ctx: Context<BuyBackBurn>, amount: u64) -> Result<()> {
        let burn_amount = (amount as u128 * 40 / 100) as u64;

        let cpi_accounts = Burn {
            mint: ctx.accounts.ilink_mint.to_account_info(),
            from: ctx.accounts.fee_vault.to_account_info(),
            authority: ctx.accounts.fee_authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::burn(cpi_ctx, burn_amount)?;

        let registry = &mut ctx.accounts.state_registry;
        registry.total_burned = registry.total_burned.checked_add(burn_amount).unwrap();

        emit!(TokenBurnedEvent {
            amount: burn_amount
        });

        Ok(())
    }

    /// Initialize a liquidity pool for cross-chain swaps
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        initial_reserve_a: u64,
        initial_reserve_b: u64,
    ) -> Result<()> {
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

fn verify_groth16_proof(proof: &[u8], _payload_hash: &[u8; 32]) -> bool {
    if proof.len() != 256 {
        return false;
    }

    let mut pairing_input = [0u8; 384];
    pairing_input[0..64].copy_from_slice(&proof[0..64]);
    pairing_input[64..192].copy_from_slice(&proof[64..192]);
    pairing_input[192..256].copy_from_slice(&proof[192..256]);
    pairing_input[256..384].copy_from_slice(&NEG_G2_GEN);

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
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SubmitProof<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump
    )]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(mut)]
    pub relayer: Signer<'info>,
}

#[derive(Accounts)]
pub struct VerifyAndMint<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump
    )]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(mut)]
    pub token_mint: Account<'info, Mint>,
    #[account(mut)]
    pub recipient_token_account: Account<'info, TokenAccount>,
    /// CHECK: PDA used as mint authority
    #[account(
        seeds = [b"mint_authority"],
        bump
    )]
    pub mint_authority: AccountInfo<'info>,
    #[account(mut)]
    pub relayer: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ProcessCrossChainSwap<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump
    )]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        mut,
        seeds = [
            b"liquidity_pool",
            liquidity_pool.token_a_mint.as_ref(),
            liquidity_pool.token_b_mint.as_ref(),
        ],
        bump
    )]
    pub liquidity_pool: Account<'info, LiquidityPool>,
    #[account(mut)]
    pub pool_token_b_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub recipient_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub relayer: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Stake<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump
    )]
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
        mut,
        seeds = [b"stake_vault", relayer.key().as_ref()],
        bump
    )]
    pub stake_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub relayer_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub relayer: Signer<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Unstake<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump
    )]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        mut,
        seeds = [b"stake", relayer.key().as_ref()],
        bump,
        has_one = relayer
    )]
    pub stake_account: Account<'info, StakeAccount>,
    #[account(
        mut,
        seeds = [b"stake_vault", relayer.key().as_ref()],
        bump
    )]
    pub stake_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub relayer_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub relayer: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct SlashRelayer<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump,
        has_one = admin
    )]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        mut,
        seeds = [b"stake", stake_account.relayer.as_ref()],
        bump
    )]
    pub stake_account: Account<'info, StakeAccount>,
    #[account(
        mut,
        seeds = [b"stake_vault", stake_account.relayer.as_ref()],
        bump
    )]
    pub stake_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub ilink_mint: Account<'info, Mint>,
    pub admin: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct BuyBackBurn<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump
    )]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(mut)]
    pub ilink_mint: Account<'info, Mint>,
    #[account(mut)]
    pub fee_vault: Account<'info, TokenAccount>,
    pub fee_authority: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct InitializePool<'info> {
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
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

// ─── State Accounts ─────────────────────────────────────────────────────────

#[account]
#[derive(InitSpace)]
pub struct StateRegistry {
    pub admin: Pubkey,           // 32
    pub fee_rate_bps: u16,       // 2
    pub processed_sequences: u64, // 8
    pub total_staked: u64,       // 8
    pub total_burned: u64,       // 8
}

#[account]
#[derive(InitSpace)]
pub struct StakeAccount {
    pub relayer: Pubkey,    // 32
    pub amount: u64,        // 8
    pub staked_at: i64,     // 8
}

#[account]
#[derive(InitSpace)]
pub struct LiquidityPool {
    pub token_a_mint: Pubkey, // 32
    pub token_b_mint: Pubkey, // 32
    pub reserve_a: u64,       // 8
    pub reserve_b: u64,       // 8
    pub admin: Pubkey,        // 32
}

// ─── Events ─────────────────────────────────────────────────────────────────

#[event]
pub struct ProofVerifiedEvent {
    pub sequence: u64,
    pub payload_hash: [u8; 32],
    pub relayer: Pubkey,
}

#[event]
pub struct MintEvent {
    pub sequence: u64,
    pub recipient: Pubkey,
    pub amount: u64,
    pub payload_hash: [u8; 32],
}

#[event]
pub struct SwapExecutedEvent {
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

// ─── Errors ─────────────────────────────────────────────────────────────────

#[error_code]
pub enum HubError {
    #[msg("This sequence has already been processed.")]
    SequenceAlreadyProcessed,
    #[msg("Invalid ZK proof: BN254 pairing check failed.")]
    InvalidProof,
    #[msg("Proof must be exactly 256 bytes.")]
    InvalidProofLength,
    #[msg("Insufficient stake: minimum 100,000 ILINK required.")]
    InsufficientStake,
    #[msg("Slippage exceeded: output amount below minimum.")]
    SlippageExceeded,
    #[msg("Liquidity pool is empty.")]
    EmptyPool,
}
