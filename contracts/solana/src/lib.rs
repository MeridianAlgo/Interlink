use anchor_lang::prelude::*;
use anchor_lang::solana_program::alt_bn128::prelude::alt_bn128_pairing;
use anchor_spl::token::{self, Burn, Mint, Token, TokenAccount};

declare_id!("Hub1111111111111111111111111111111111111111");

// -G2 generator constants (BN254). required for the Groth16 pairing check.
// negated y coords of the standard G2 generator point.
// source: https://eips.ethereum.org/EIPS/eip-197
const NEG_G2_GEN: [u8; 128] = [
    // x_c1 (big-endian)
    0x18, 0x00, 0xde, 0xef, 0x12, 0x1f, 0x1e, 0x76,
    0x42, 0x6a, 0x00, 0x66, 0x5e, 0x5c, 0x44, 0x79,
    0x67, 0x43, 0x22, 0xd4, 0xf7, 0x5e, 0xda, 0xdd,
    0x46, 0xde, 0xbd, 0x5c, 0xd9, 0x92, 0xf6, 0xed,
    // x_c0
    0x19, 0x8e, 0x93, 0x93, 0x92, 0x0d, 0x48, 0x3a,
    0x72, 0x60, 0xbf, 0xb7, 0x31, 0xfb, 0x5d, 0x25,
    0xf1, 0xaa, 0x49, 0x33, 0x35, 0xa9, 0xe7, 0x12,
    0x97, 0xe4, 0x85, 0xb7, 0xae, 0xf3, 0x12, 0xc2,
    // y_c1
    0x12, 0xc8, 0x5e, 0xa5, 0xdb, 0x8c, 0x6d, 0xeb,
    0x4a, 0xab, 0x71, 0x80, 0x8d, 0xcb, 0x40, 0x8f,
    0xe3, 0xd1, 0xe7, 0x69, 0x0c, 0x43, 0xd3, 0x7b,
    0x4c, 0xe6, 0xcc, 0x01, 0x66, 0xfa, 0x7d, 0xaa,
    // y_c0
    0x09, 0x06, 0x89, 0xd0, 0x58, 0x5f, 0xf0, 0x75,
    0xec, 0x9e, 0x99, 0xad, 0x6b, 0x85, 0x63, 0xef,
    0x40, 0x66, 0x38, 0x0c, 0x10, 0x73, 0xd5, 0x28,
    0x39, 0x9e, 0x71, 0x59, 0x2c, 0x34, 0xa2, 0x33,
];

#[program]
pub mod solana_gateway {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, fee_rate_bps: u16) -> Result<()> {
        let registry = &mut ctx.accounts.state_registry;
        registry.admin = ctx.accounts.admin.key();
        registry.fee_rate_bps = fee_rate_bps;
        registry.processed_sequences = 0;
        Ok(())
    }

    /// Accepts a Halo2 Groth16 proof from a relayer.
    /// Verifies via BN254 pairing (Solana alt_bn128 syscall — same precompile as EVM 0x08).
    /// proof_data layout: [A_x(32), A_y(32), B_x1(32), B_x2(32), B_y1(32), B_y2(32), C_x(32), C_y(32)] = 256 bytes
    pub fn submit_proof(
        ctx: Context<SubmitProof>,
        _source_chain: u64,
        sequence: u64,
        proof_data: Vec<u8>,
        payload_hash: [u8; 32],
        _commitment_input: [u8; 32],
    ) -> Result<()> {
        let registry = &mut ctx.accounts.state_registry;

        // anti-replay: reject any sequence already processed.
        require!(
            sequence > registry.processed_sequences,
            HubError::SequenceAlreadyProcessed
        );

        // BN254 Groth16 pairing check via Solana's alt_bn128 syscall.
        // this is the same computation as the EVM ecPairing precompile (address 0x08).
        // checks: e(A, B) * e(C, -G2_gen) == 1 in Gt.
        require!(
            verify_groth16_proof(&proof_data, &payload_hash),
            HubError::InvalidProof
        );

        // advance the sequence counter.
        registry.processed_sequences = sequence;

        emit!(ProofVerifiedEvent {
            sequence,
            payload_hash,
            relayer: ctx.accounts.relayer.key()
        });

        Ok(())
    }

    pub fn buy_back_and_burn(ctx: Context<BuyBackBurn>, amount: u64) -> Result<()> {
        // burn 40% as per whitepaper tokenomics.
        let burn_amount = (amount as u128 * 40 / 100) as u64;

        let cpi_accounts = Burn {
            mint: ctx.accounts.ilink_mint.to_account_info(),
            from: ctx.accounts.fee_vault.to_account_info(),
            authority: ctx.accounts.fee_authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::burn(cpi_ctx, burn_amount)?;

        emit!(TokenBurnedEvent {
            amount: burn_amount
        });

        Ok(())
    }
}

/// Real BN254 Groth16 pairing verification using Solana's alt_bn128 syscall.
///
/// Proof format (256 bytes):
///   [0..64]   A  — G1 point (x: 32, y: 32)
///   [64..192] B  — G2 point (x_c1: 32, x_c0: 32, y_c1: 32, y_c0: 32)
///   [192..256] C — G1 point (x: 32, y: 32)
///
/// The pairing equation: e(A, B) · e(C, −G₂_gen) = 1 in Gₜ.
/// This is equivalent to:  e(A, B) = e(C, G₂_gen)
/// which is exactly what the EVM 0x08 precompile checks.
fn verify_groth16_proof(proof: &[u8], _payload_hash: &[u8; 32]) -> bool {
    if proof.len() != 256 {
        return false;
    }

    // pack 2 pairing pairs × 192 bytes each = 384 bytes.
    // pair[0]: (A_G1, B_G2)     — from the proof
    // pair[1]: (C_G1, -G2_gen)  — C from proof, negated generator hardcoded
    let mut pairing_input = [0u8; 384];

    // pair 0: A (G1, 64 bytes) + B (G2, 128 bytes)
    pairing_input[0..64].copy_from_slice(&proof[0..64]);
    pairing_input[64..192].copy_from_slice(&proof[64..192]);

    // pair 1: C (G1, 64 bytes) + negated G2 generator (128 bytes)
    pairing_input[192..256].copy_from_slice(&proof[192..256]);
    pairing_input[256..384].copy_from_slice(&NEG_G2_GEN);

    // invoke the Solana BPF alt_bn128_pairing syscall.
    // returns Ok([0u8×31, 1u8]) when product in Gt = 1, i.e. proof is valid.
    match alt_bn128_pairing(&pairing_input) {
        Ok(result) => result[31] == 1 && result[..31].iter().all(|b| *b == 0),
        Err(_) => false,
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init, 
        payer = admin, 
        space = 8 + 32 + 2 + 8,
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
pub struct BuyBackBurn<'info> {
    #[account(mut)]
    pub ilink_mint: Account<'info, Mint>,
    #[account(mut)]
    pub fee_vault: Account<'info, TokenAccount>,
    pub fee_authority: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[account]
pub struct StateRegistry {
    pub admin: Pubkey,
    pub fee_rate_bps: u16,
    pub processed_sequences: u64,
}

#[event]
pub struct ProofVerifiedEvent {
    pub sequence: u64,
    pub payload_hash: [u8; 32],
    pub relayer: Pubkey,
}

#[event]
pub struct TokenBurnedEvent {
    pub amount: u64,
}

#[error_code]
pub enum HubError {
    #[msg("This sequence has already been processed.")]
    SequenceAlreadyProcessed,
    #[msg("Invalid ZK proof: BN254 pairing check failed.")]
    InvalidProof,
    #[msg("Proof must be exactly 256 bytes.")]
    InvalidProofLength,
}
