use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{transfer_checked, Mint, Token, TokenAccount, TransferChecked};

use crate::errors::AmmError;
use crate::state::Config;

#[derive(Accounts)]
pub struct Claim<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    pub mint_x: Box<Account<'info, Mint>>,

    pub mint_y: Box<Account<'info, Mint>>,

    #[account(
        has_one = authority,
        has_one = mint_x,
        has_one = mint_y,
        seeds = [b"config", config.seed.to_le_bytes().as_ref()],
        bump = config.config_bump
    )]
    pub config: Account<'info, Config>,

    #[account(
        mut,
        seeds = [b"treasury_x", config.key().as_ref()],
        bump
    )]
    pub treasury_x: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [b"treasury_y", config.key().as_ref()],
        bump
    )]
    pub treasury_y: Box<Account<'info, TokenAccount>>,

    #[account(
        init_if_needed,
        payer = authority,
        associated_token::mint = mint_x,
        associated_token::authority = authority
    )]
    pub authority_x: Box<Account<'info, TokenAccount>>,

    #[account(
        init_if_needed,
        payer = authority,
        associated_token::mint = mint_y,
        associated_token::authority = authority
    )]
    pub authority_y: Box<Account<'info, TokenAccount>>,

    pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

impl<'info> Claim<'info> {
    pub fn claim(&mut self) -> Result<()> {
        let amount_x = self.treasury_x.amount;
        let amount_y = self.treasury_y.amount;

        require!(amount_x > 0 || amount_y > 0, AmmError::NothingToClaim);

        if amount_x > 0 {
            self.sweep(true, amount_x)?;
        }

        if amount_y > 0 {
            self.sweep(false, amount_y)?;
        }

        Ok(())
    }

    fn sweep(&self, is_x: bool, amount: u64) -> Result<()> {
        let (from, to, mint, decimals) = match is_x {
            true => (
                self.treasury_x.to_account_info(),
                self.authority_x.to_account_info(),
                self.mint_x.to_account_info(),
                self.mint_x.decimals,
            ),
            false => (
                self.treasury_y.to_account_info(),
                self.authority_y.to_account_info(),
                self.mint_y.to_account_info(),
                self.mint_y.decimals,
            ),
        };

        let seed_bytes = self.config.seed.to_le_bytes();
        let seeds = &[
            b"config".as_ref(),
            seed_bytes.as_ref(),
            &[self.config.config_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = TransferChecked {
            from,
            mint,
            to,
            authority: self.config.to_account_info(),
        };

        transfer_checked(
            CpiContext::new_with_signer(self.token_program.key(), cpi_accounts, signer_seeds),
            amount,
            decimals,
        )
    }
}
