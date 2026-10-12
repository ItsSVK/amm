use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::errors::AmmError;
use crate::state::Config;

#[derive(Accounts)]
#[instruction(seed: u64)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    pub mint_x: Box<Account<'info, Mint>>,

    pub mint_y: Box<Account<'info, Mint>>,

    #[account(
        init,
        payer = authority,
        space = 8 + Config::INIT_SPACE,
        seeds = [b"config", seed.to_le_bytes().as_ref()],
        bump
    )]
    pub config: Account<'info, Config>,

    #[account(
        init,
        payer = authority,
        seeds = [b"lp", config.key().as_ref()],
        bump,
        mint::decimals = 6,
        mint::authority = config
    )]
    pub mint_lp: Box<Account<'info, Mint>>,

    #[account(
        init,
        payer = authority,
        associated_token::mint = mint_x,
        associated_token::authority = config
    )]
    pub vault_x: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = authority,
        associated_token::mint = mint_y,
        associated_token::authority = config
    )]
    pub vault_y: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = authority,
        seeds = [b"treasury_x", config.key().as_ref()],
        bump,
        token::mint = mint_x,
        token::authority = config
    )]
    pub treasury_x: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = authority,
        seeds = [b"treasury_y", config.key().as_ref()],
        bump,
        token::mint = mint_y,
        token::authority = config
    )]
    pub treasury_y: Box<Account<'info, TokenAccount>>,

    pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

impl<'info> Initialize<'info> {
    pub fn initialize(
        &mut self,
        seed: u64,
        fee: u16,
        protocol_fee: u16,
        bumps: &InitializeBumps,
    ) -> Result<()> {
        require!(fee < 10_000, AmmError::InvalidFee);
        require!(protocol_fee < 10_000, AmmError::InvalidFee);
        require_keys_neq!(
            self.mint_x.key(),
            self.mint_y.key(),
            AmmError::IdenticalMints
        );

        self.config.set_inner(Config {
            seed,
            authority: self.authority.key(),
            mint_x: self.mint_x.key(),
            mint_y: self.mint_y.key(),
            fee,
            protocol_fee,
            locked: false,
            config_bump: bumps.config,
            lp_bump: bumps.mint_lp,
        });

        Ok(())
    }
}
