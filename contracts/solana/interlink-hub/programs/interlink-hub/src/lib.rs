use anchor_lang::prelude::*;

declare_id!("AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz");

#[program]
pub mod interlink_hub {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        msg!("Greetings from: {:?}", ctx.program_id);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
