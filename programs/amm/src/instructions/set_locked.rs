use anchor_lang::prelude::*;

use crate::state::Config;

#[derive(Accounts)]
pub struct SetLocked<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        has_one = authority,
        seeds = [b"config", config.seed.to_le_bytes().as_ref()],
        bump = config.config_bump
    )]
    pub config: Account<'info, Config>,
}

impl<'info> SetLocked<'info> {
    pub fn set_locked(&mut self, locked: bool) -> Result<()> {
        self.config.locked = locked;

        Ok(())
    }
}
