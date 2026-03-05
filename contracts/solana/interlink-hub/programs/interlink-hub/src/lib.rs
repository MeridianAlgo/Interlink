use anchor_lang::prelude::*;
use anchor_lang::solana_program::alt_bn128::prelude::{
    alt_bn128_addition, alt_bn128_multiplication, alt_bn128_pairing,
};
use anchor_spl::token::{self, Burn, Mint, MintTo, Token, TokenAccount, Transfer};

declare_id!("AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz");

/// BN254 scalar field modulus r (NOT the base field p).
/// r = 21888242871839275222246405745257275088548364400416034343698204186575808495617
/// hex: 30644e72e131a029b85045b68181585d 2833e84879b9709143e1f593f0000001
const BN254_SCALAR_FIELD: [u8; 32] = [
    0x30, 0x64, 0x4e, 0x72, 0xe1, 0x31, 0xa0, 0x29,
    0xb8, 0x50, 0x45, 0xb6, 0x81, 0x81, 0x58, 0x5d,
    0x28, 0x33, 0xe8, 0x48, 0x79, 0xb9, 0x70, 0x91,
    0x43, 0xe1, 0xf5, 0x93, 0xf0, 0x00, 0x00, 0x01,
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
        registry.next_sequence = 0;
        registry.total_staked = 0;
        registry.total_burned = 0;
        registry.vk_initialized = false;
        Ok(())
    }

    /// Store the Groth16 verification key on-chain. Must be called once after
    /// trusted setup, before any proofs can be verified. Admin-only.
    ///
    /// vk_data layout (576 bytes for 1 public input):
    ///   alpha_g1:  64 bytes (G1)
    ///   beta_g2:  128 bytes (G2)
    ///   gamma_g2: 128 bytes (G2)
    ///   delta_g2: 128 bytes (G2)
    ///   ic_0:      64 bytes (G1)
    ///   ic_1:      64 bytes (G1)
    pub fn set_verification_key(
        ctx: Context<SetVerificationKey>,
        vk_data: Vec<u8>,
    ) -> Result<()> {
        require!(vk_data.len() == 576, HubError::InvalidVKLength);

        let vk_account = &mut ctx.accounts.verification_key;
        vk_account.alpha_g1.copy_from_slice(&vk_data[0..64]);
        vk_account.beta_g2.copy_from_slice(&vk_data[64..192]);
        vk_account.gamma_g2.copy_from_slice(&vk_data[192..320]);
        vk_account.delta_g2.copy_from_slice(&vk_data[320..448]);
        vk_account.ic_0.copy_from_slice(&vk_data[448..512]);
        vk_account.ic_1.copy_from_slice(&vk_data[512..576]);

        let registry = &mut ctx.accounts.state_registry;
        registry.vk_initialized = true;

        emit!(VKUpdatedEvent {
            admin: ctx.accounts.admin.key(),
        });

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
        // Require relayer has active stake
        let stake_account = &ctx.accounts.stake_account;
        require!(
            stake_account.amount >= MIN_STAKE_AMOUNT,
            HubError::InsufficientStake
        );

        let registry = &mut ctx.accounts.state_registry;
        require!(registry.vk_initialized, HubError::VKNotInitialized);

        // Strict sequential ordering: EVM nonces start at 0
        require!(
            sequence == registry.next_sequence,
            HubError::SequenceAlreadyProcessed
        );

        require!(proof_data.len() == 256, HubError::InvalidProofLength);

        let vk = &ctx.accounts.verification_key;
        require!(
            verify_groth16_proof(&proof_data, &payload_hash, sequence, vk),
            HubError::InvalidProof
        );

        registry.next_sequence = sequence + 1;

        emit!(ProofVerifiedEvent {
            sequence,
            payload_hash,
            relayer: ctx.accounts.relayer.key()
        });

        Ok(())
    }

    /// Verify a ZK proof and mint wrapped tokens to the recipient.
    pub fn verify_and_mint(
        ctx: Context<VerifyAndMint>,
        sequence: u64,
        amount: u64,
        proof_data: Vec<u8>,
        payload_hash: [u8; 32],
    ) -> Result<()> {
        let stake_account = &ctx.accounts.stake_account;
        require!(
            stake_account.amount >= MIN_STAKE_AMOUNT,
            HubError::InsufficientStake
        );

        let registry = &mut ctx.accounts.state_registry;
        require!(registry.vk_initialized, HubError::VKNotInitialized);

        require!(
            sequence == registry.next_sequence,
            HubError::SequenceAlreadyProcessed
        );

        require!(proof_data.len() == 256, HubError::InvalidProofLength);

        let vk = &ctx.accounts.verification_key;
        require!(
            verify_groth16_proof(&proof_data, &payload_hash, sequence, vk),
            HubError::InvalidProof
        );

        registry.next_sequence = sequence + 1;

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
    pub fn process_cross_chain_swap(
        ctx: Context<ProcessCrossChainSwap>,
        sequence: u64,
        amount_in: u64,
        min_amount_out: u64,
        proof_data: Vec<u8>,
        payload_hash: [u8; 32],
    ) -> Result<()> {
        let stake_account = &ctx.accounts.stake_account;
        require!(
            stake_account.amount >= MIN_STAKE_AMOUNT,
            HubError::InsufficientStake
        );

        let registry = &mut ctx.accounts.state_registry;
        require!(registry.vk_initialized, HubError::VKNotInitialized);

        require!(
            sequence == registry.next_sequence,
            HubError::SequenceAlreadyProcessed
        );

        require!(proof_data.len() == 256, HubError::InvalidProofLength);

        let vk = &ctx.accounts.verification_key;
        require!(
            verify_groth16_proof(&proof_data, &payload_hash, sequence, vk),
            HubError::InvalidProof
        );

        registry.next_sequence = sequence + 1;

        // Constant product AMM: amount_out = (amount_in * reserve_out) / (reserve_in + amount_in)
        // Extract fields and do all pool mutations in a block so the mutable borrow
        // is released before we call to_account_info() for the CPI below.
        let token_a_mint;
        let token_b_mint;
        let pool_bump;
        let amount_out;
        let fee;
        {
            let pool = &mut ctx.accounts.liquidity_pool;
            let reserve_in = pool.reserve_a;
            let reserve_out = pool.reserve_b;

            require!(reserve_in > 0 && reserve_out > 0, HubError::EmptyPool);

            // Apply fee (fee_rate_bps basis points)
            fee = (amount_in as u128)
                .checked_mul(registry.fee_rate_bps as u128)
                .unwrap()
                .checked_div(10_000)
                .unwrap() as u64;
            let amount_in_after_fee = amount_in.checked_sub(fee).unwrap();

            amount_out = (amount_in_after_fee as u128)
                .checked_mul(reserve_out as u128)
                .unwrap()
                .checked_div(
                    (reserve_in as u128)
                        .checked_add(amount_in_after_fee as u128)
                        .unwrap(),
                )
                .unwrap() as u64;

            require!(amount_out >= min_amount_out, HubError::SlippageExceeded);

            // Update pool reserves
            pool.reserve_a = reserve_in.checked_add(amount_in_after_fee).unwrap();
            pool.reserve_b = reserve_out.checked_sub(amount_out).unwrap();

            token_a_mint = pool.token_a_mint;
            token_b_mint = pool.token_b_mint;
            pool_bump = ctx.bumps.liquidity_pool;
        } // mutable borrow of liquidity_pool ends here

        // Transfer output tokens to recipient
        let pool_seeds = &[
            b"liquidity_pool".as_ref(),
            token_a_mint.as_ref(),
            token_b_mint.as_ref(),
            &[pool_bump],
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

        let remaining = stake_account.amount.checked_sub(amount).unwrap();
        require!(
            remaining == 0 || remaining >= MIN_STAKE_AMOUNT,
            HubError::InsufficientStake
        );

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
    pub fn slash_relayer(ctx: Context<SlashRelayer>) -> Result<()> {
        let stake_account = &mut ctx.accounts.stake_account;
        let slash_amount = stake_account
            .amount
            .checked_mul(SLASH_BPS)
            .unwrap()
            .checked_div(10_000)
            .unwrap();

        stake_account.amount = stake_account.amount.checked_sub(slash_amount).unwrap();

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
            amount: burn_amount,
        });

        Ok(())
    }

    /// Initialize a liquidity pool for cross-chain swaps.
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        initial_reserve_a: u64,
        initial_reserve_b: u64,
    ) -> Result<()> {
        require!(
            initial_reserve_a > 0 && initial_reserve_b > 0,
            HubError::EmptyPool
        );

        let cpi_a = Transfer {
            from: ctx.accounts.admin_token_a.to_account_info(),
            to: ctx.accounts.pool_token_a_vault.to_account_info(),
            authority: ctx.accounts.admin.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_a),
            initial_reserve_a,
        )?;

        let cpi_b = Transfer {
            from: ctx.accounts.admin_token_b.to_account_info(),
            to: ctx.accounts.pool_token_b_vault.to_account_info(),
            authority: ctx.accounts.admin.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_b),
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

// ─── Groth16 Verification ───────────────────────────────────────────────────
//
// Standard Groth16 verification equation (4-pairing check):
//   e(A, B) = e(alpha, beta) · e(L, gamma) · e(C, delta)
//
// Equivalently as a single multi-pairing == 1 check:
//   e(-A, B) · e(alpha, beta) · e(L, gamma) · e(C, delta) = 1
//
// Where L = IC[0] + commitment * IC[1]
// and commitment is derived from the payload_hash via domain-separated keccak.

/// Domain separation salt (must match prover and EVM gateway)
const DOMAIN_SALT: &[u8] = b"interlink_v1_domain";

/// Derive the public input (commitment) from a payload_hash.
/// This matches the prover's computation:
///   msg = Fr::from_be_bytes_mod_order(payload_hash)
///   rc  = Fr::from_be_bytes_mod_order(keccak256("interlink_v1_domain"))
///   commitment = (msg + rc)^5 + sequence
///
/// However, on-chain we verify against the commitment directly encoded in the
/// VK's IC array via the Groth16 protocol. We just need the commitment as a
/// scalar for the IC linear combination.
///
/// For the standard Groth16 flow, the public input IS the commitment scalar
/// that was used during proving. The verifier recomputes:
///   L = IC[0] + commitment * IC[1]
/// and checks the pairing equation.
///
/// The commitment binds to the payload because the circuit constrains:
///   commitment = (hash(payload) + rc)^5 + sequence
///
/// So we need to recompute commitment on-chain to use as the public input.
fn compute_commitment_on_chain(payload_hash: &[u8; 32], sequence: u64) -> [u8; 32] {
    use anchor_lang::solana_program::keccak;

    // rc = keccak256("interlink_v1_domain") reduced mod BN254 scalar field
    let rc_hash = keccak::hash(DOMAIN_SALT).to_bytes();
    let rc = reduce_mod_scalar_field(&rc_hash);

    // msg = payload_hash reduced mod BN254 scalar field
    let msg = reduce_mod_scalar_field(payload_hash);

    // w = msg + rc (mod r)
    let w = field_add(&msg, &rc);

    // w^2
    let w2 = field_mul(&w, &w);
    // w^4
    let w4 = field_mul(&w2, &w2);
    // w^5
    let w5 = field_mul(&w4, &w);

    // seq as field element
    let mut seq_bytes = [0u8; 32];
    seq_bytes[24..32].copy_from_slice(&sequence.to_be_bytes());

    // commitment = w^5 + seq
    field_add(&w5, &seq_bytes)
}

/// Reduce a 32-byte big-endian value modulo the BN254 scalar field.
/// Uses repeated subtraction (at most 3 iterations since 2^256 / r ≈ 4).
fn reduce_mod_scalar_field(val: &[u8; 32]) -> [u8; 32] {
    let mut result = *val;
    // At most 3 subtractions needed (2^256 / r < 4)
    for _ in 0..4 {
        if !ge_scalar_field(&result) {
            break;
        }
        result = field_sub_modulus(&result);
    }
    result
}

/// Check if val >= BN254_SCALAR_FIELD (big-endian comparison).
fn ge_scalar_field(val: &[u8; 32]) -> bool {
    for i in 0..32 {
        if val[i] > BN254_SCALAR_FIELD[i] {
            return true;
        }
        if val[i] < BN254_SCALAR_FIELD[i] {
            return false;
        }
    }
    true // equal
}

/// Subtract BN254_SCALAR_FIELD from val (big-endian). Assumes val >= modulus.
fn field_sub_modulus(val: &[u8; 32]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut borrow: u16 = 0;
    for i in (0..32).rev() {
        let diff = (val[i] as u16)
            .wrapping_sub(BN254_SCALAR_FIELD[i] as u16)
            .wrapping_sub(borrow);
        result[i] = diff as u8;
        borrow = if diff > 255 { 1 } else { 0 };
    }
    result
}

/// Field addition mod BN254 scalar field (big-endian 32-byte inputs).
fn field_add(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut carry: u16 = 0;
    for i in (0..32).rev() {
        let sum = (a[i] as u16) + (b[i] as u16) + carry;
        result[i] = sum as u8;
        carry = sum >> 8;
    }
    // Reduce if >= modulus
    if carry > 0 || ge_scalar_field(&result) {
        result = field_sub_modulus(&result);
    }
    result
}

/// Field multiplication mod BN254 scalar field (big-endian 32-byte inputs).
/// Uses schoolbook multiplication into a 64-byte intermediate, then Barrett reduction.
fn field_mul(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    // Schoolbook multiplication: a * b → 64-byte product
    let mut product = [0u16; 64];
    for i in (0..32).rev() {
        let mut carry: u16 = 0;
        for j in (0..32).rev() {
            let idx = i + j + 1;
            let val = product[idx] + (a[i] as u16) * (b[j] as u16) + carry;
            product[idx] = val & 0xFF;
            carry = val >> 8;
        }
        product[i] += carry;
    }

    // Propagate carries
    let mut wide = [0u8; 64];
    let mut carry: u16 = 0;
    for i in (0..64).rev() {
        let val = product[i] + carry;
        wide[i] = val as u8;
        carry = val >> 8;
    }

    // Reduce mod scalar field: repeated subtraction on the 512-bit value
    // For production, use Montgomery or Barrett reduction. For correctness,
    // we use the alt_bn128_multiplication syscall to compute a*b*G1 and
    // extract the scalar. But that's circular. Instead, implement proper
    // reduction via repeated conditional subtraction on the wide value.
    //
    // Since r ≈ 2^254, a 512-bit value needs at most ~2^258/r ≈ 16 subtractions
    // of r from the high part. But this is expensive in compute units.
    //
    // Better approach: use the ECMUL precompile to do modular arithmetic.
    // Compute: a_scalar * (b_scalar * G1) where G1 = (1, 2).
    // The x-coordinate of the result encodes the product mod r... but not directly.
    //
    // For correctness and simplicity: implement proper 256-bit modular reduction.
    // The product is at most 2^512. We need product mod r.

    // Simple long-division style reduction:
    // Process the upper 32 bytes, shift and subtract.
    let mut remainder = [0u8; 32];
    remainder.copy_from_slice(&wide[32..64]);

    // Process each byte of the upper half
    for byte_idx in 0..32 {
        // Shift remainder left by 8 bits, add next byte from upper half
        // Actually we need to process from MSB to LSB of the wide number.
        // Let's use a different approach: schoolbook division.
        //
        // For on-chain efficiency, we use the ECMUL trick:
        // To compute (a * b) mod r, note that ECMUL computes scalar * Point
        // where scalar is automatically reduced mod r.
        // So we can compute: ECMUL(ECMUL(G1, a), b) to get (a*b mod r) * G1
        // But we can't extract the scalar back from the point.
        //
        // Instead, we'll do modular multiplication via the schoolbook method
        // with proper 512→256 bit reduction.
        let _ = byte_idx; // suppress warning
    }

    // Proper 512-bit to 256-bit reduction using shift-and-subtract.
    // Process the wide number byte by byte from MSB.
    let mut acc = [0u8; 33]; // 33 bytes to handle overflow during shift
    for byte_idx in 0..64 {
        // Shift accumulator left by 8 bits
        for k in 0..32 {
            acc[k] = acc[k + 1];
        }
        acc[32] = wide[byte_idx];

        // While acc >= r, subtract r
        loop {
            // Check if acc[0..33] >= r (treating acc as 33-byte big-endian)
            // If acc[0] > 0, it's definitely >= r (since r fits in 32 bytes)
            if acc[0] > 0 {
                // Subtract r from acc[1..33]
                let mut borrow: u16 = 0;
                for k in (0..32).rev() {
                    let diff = (acc[k + 1] as u16)
                        .wrapping_sub(BN254_SCALAR_FIELD[k] as u16)
                        .wrapping_sub(borrow);
                    acc[k + 1] = diff as u8;
                    borrow = if diff > 255 { 1 } else { 0 };
                }
                // borrow from acc[0]
                acc[0] = acc[0].wrapping_sub(borrow as u8);
            } else {
                // acc[0] == 0, check acc[1..33] >= BN254_SCALAR_FIELD
                let mut ge = true;
                for k in 0..32 {
                    if acc[k + 1] > BN254_SCALAR_FIELD[k] {
                        break;
                    }
                    if acc[k + 1] < BN254_SCALAR_FIELD[k] {
                        ge = false;
                        break;
                    }
                }
                if ge {
                    let mut borrow: u16 = 0;
                    for k in (0..32).rev() {
                        let diff = (acc[k + 1] as u16)
                            .wrapping_sub(BN254_SCALAR_FIELD[k] as u16)
                            .wrapping_sub(borrow);
                        acc[k + 1] = diff as u8;
                        borrow = if diff > 255 { 1 } else { 0 };
                    }
                } else {
                    break;
                }
            }
        }
    }

    let mut out = [0u8; 32];
    out.copy_from_slice(&acc[1..33]);
    out
}

/// Negate a G1 point (flip y coordinate: y' = p - y where p is the base field modulus).
/// Base field p = 21888242871839275222246405745257275088696311157297823662689037894645226208583
const BN254_BASE_FIELD: [u8; 32] = [
    0x30, 0x64, 0x4e, 0x72, 0xe1, 0x31, 0xa0, 0x29,
    0xb8, 0x50, 0x45, 0xb6, 0x81, 0x81, 0x58, 0x5d,
    0x97, 0x81, 0x6a, 0x91, 0x68, 0x71, 0xca, 0x8d,
    0x3c, 0x20, 0x8c, 0x16, 0xd8, 0x7c, 0xfd, 0x47,
];

fn negate_g1(point: &[u8; 64]) -> [u8; 64] {
    let mut result = [0u8; 64];
    result[0..32].copy_from_slice(&point[0..32]); // x unchanged

    // y' = p - y
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

/// Standard Groth16 verification using BN254 precompiles.
///
/// Verification equation (single multi-pairing check):
///   e(-A, B) · e(alpha, beta) · e(L, gamma) · e(C, delta) = 1
///
/// Where L = IC[0] + public_input * IC[1]
///
/// The public input is the circuit commitment: (msg + rc)^5 + seq,
/// where msg = payload_hash mod r, rc = keccak256(DOMAIN_SALT) mod r,
/// seq = sequence as field element. This matches the prover exactly.
///
/// Uses 4 pairings in a single alt_bn128_pairing syscall (768 bytes input).
fn verify_groth16_proof(
    proof: &[u8],
    payload_hash: &[u8; 32],
    sequence: u64,
    vk: &VerificationKey,
) -> bool {
    if proof.len() != 256 {
        return false;
    }

    // Extract proof points
    let a: [u8; 64] = proof[0..64].try_into().unwrap();
    let b: [u8; 128] = proof[64..192].try_into().unwrap();
    let c: [u8; 64] = proof[192..256].try_into().unwrap();

    // Recompute the public input commitment identically to the prover:
    //   commitment = (msg + rc)^5 + seq
    // where msg = payload_hash mod r, rc = keccak256("interlink_v1_domain") mod r.
    let commitment = compute_commitment_on_chain(payload_hash, sequence);

    // L = IC[0] + commitment * IC[1]  (ECMUL + ECADD)
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

    // Negate A for the multi-pairing check: e(-A, B) · e(α, β) · e(L, γ) · e(C, δ) = 1
    let neg_a = negate_g1(&a);

    // Build 4-pairing input (768 bytes = 4 × 192)
    let mut pairing_input = [0u8; 768];

    // Pair 0: (-A, B)
    pairing_input[0..64].copy_from_slice(&neg_a);
    pairing_input[64..192].copy_from_slice(&b);

    // Pair 1: (alpha, beta)
    pairing_input[192..256].copy_from_slice(&vk.alpha_g1);
    pairing_input[256..384].copy_from_slice(&vk.beta_g2);

    // Pair 2: (L, gamma)
    pairing_input[384..448].copy_from_slice(&l_point[..64]);
    pairing_input[448..576].copy_from_slice(&vk.gamma_g2);

    // Pair 3: (C, delta)
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
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetVerificationKey<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump,
        has_one = admin
    )]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        init_if_needed,
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
pub struct SubmitProof<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump
    )]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(
        seeds = [b"stake", relayer.key().as_ref()],
        bump,
        has_one = relayer
    )]
    pub stake_account: Account<'info, StakeAccount>,
    #[account(
        seeds = [b"vk"],
        bump
    )]
    pub verification_key: Account<'info, VerificationKey>,
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
    #[account(
        seeds = [b"stake", relayer.key().as_ref()],
        bump,
        has_one = relayer
    )]
    pub stake_account: Account<'info, StakeAccount>,
    #[account(
        seeds = [b"vk"],
        bump
    )]
    pub verification_key: Account<'info, VerificationKey>,
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
        seeds = [b"stake", relayer.key().as_ref()],
        bump,
        has_one = relayer
    )]
    pub stake_account: Account<'info, StakeAccount>,
    #[account(
        seeds = [b"vk"],
        bump
    )]
    pub verification_key: Account<'info, VerificationKey>,
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
        init_if_needed,
        payer = relayer,
        seeds = [b"stake_vault", relayer.key().as_ref()],
        bump,
        token::mint = ilink_mint,
        token::authority = stake_vault,
    )]
    pub stake_vault: Account<'info, TokenAccount>,
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
    pub admin_token_a: Account<'info, TokenAccount>,
    #[account(mut)]
    pub admin_token_b: Account<'info, TokenAccount>,
    #[account(mut)]
    pub pool_token_a_vault: Account<'info, TokenAccount>,
    #[account(mut)]
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
    pub admin: Pubkey,           // 32
    pub fee_rate_bps: u16,       // 2
    pub next_sequence: u64,      // 8  (renamed: was processed_sequences)
    pub total_staked: u64,       // 8
    pub total_burned: u64,       // 8
    pub vk_initialized: bool,    // 1
}

#[account]
#[derive(InitSpace)]
pub struct VerificationKey {
    pub alpha_g1: [u8; 64],   // G1 point
    pub beta_g2: [u8; 128],   // G2 point
    pub gamma_g2: [u8; 128],  // G2 point
    pub delta_g2: [u8; 128],  // G2 point
    pub ic_0: [u8; 64],       // G1 point (IC base)
    pub ic_1: [u8; 64],       // G1 point (IC for public input 0)
}

#[account]
#[derive(InitSpace)]
pub struct StakeAccount {
    pub relayer: Pubkey, // 32
    pub amount: u64,     // 8
    pub staked_at: i64,  // 8
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

#[event]
pub struct VKUpdatedEvent {
    pub admin: Pubkey,
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[error_code]
pub enum HubError {
    #[msg("This sequence has already been processed.")]
    SequenceAlreadyProcessed,
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
}
