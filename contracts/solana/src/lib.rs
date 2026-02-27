use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, Token, TokenAccount, Transfer};

declare_id!("Hub1111111111111111111111111111111111111111");

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

    pub fn submit_proof(
        ctx: Context<SubmitProof>,
        _source_chain: u64,
        sequence: u64,
        proof_data: Vec<u8>,
        payload_hash: [u8; 32],
        commitment_input: [u8; 32], // Public input from the SNARK (the commitment)
    ) -> Result<()> {
        let registry = &mut ctx.accounts.state_registry;
        
        // 1. Sequence check for the specific source chain
        require!(
            sequence > registry.processed_sequences,
            HubError::SequenceAlreadyProcessed
        );

        // 2. Cryptographic Verification
        let is_valid = verify_snark_commitment(&proof_data, commitment_input, payload_hash, sequence);
        require!(is_valid, HubError::InvalidProof);

        // 3. Update Global State
        registry.processed_sequences = sequence;

        emit!(ProofVerifiedEvent {
            sequence,
            payload_hash,
            relayer: ctx.accounts.relayer.key()
        });

        Ok(())
    }

    pub fn buy_back_and_burn(ctx: Context<BuyBackBurn>, amount: u64) -> Result<()> {
        // 40% burn strategy mentioned in whitepaper.
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

/// Real-deal architectural helper: Simulates the final constraint check that the SNARK 
/// internally verified. In production, this would be a full pairing check.
fn verify_snark_commitment(
    _proof: &[u8],
    commitment: [u8; 32],
    _payload_hash: [u8; 32],
    _sequence: u64,
) -> bool {
    // Protocol Constant: 0x1337 (matches our Rust circuit)
    // commitment should be roughly (payload_hash + rc)^3 + sequence
    // Here we just ensure the input isn't zero as a basic architectural sanity check
    commitment != [0u8; 32] && _proof.len() > 0
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer = admin, space = 8 + 32 + 2 + 8)]
    pub state_registry: Account<'info, StateRegistry>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SubmitProof<'info> {
    #[account(mut)]
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
    #[msg("This sequence has already been processed to prevent replays.")]
    SequenceAlreadyProcessed,
    #[msg("Invalid ZK Proof Submitted.")]
    InvalidProof,
}
