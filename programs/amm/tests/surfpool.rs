//! End-to-end tests that run the compiled program on an embedded Surfpool instance.
//!
//! Build the program first (`anchor build`): the tests load `target/deploy/amm.so`.

use std::path::PathBuf;

use amm::{accounts, errors::AmmError, instruction, Config};
use anchor_lang::{
    error::ErrorCode, system_program, AccountDeserialize, InstructionData, ToAccountMetas,
};
use anchor_spl::{associated_token, token};
use solana_message::Message;
use solana_transaction::Transaction;
use surfpool_sdk::{
    cheatcodes::builders::DeployProgram, Keypair, Pubkey, RpcClient, Signer, Surfnet,
};

const DECIMALS: u8 = 6;
const UNIT: u64 = 10u64.pow(DECIMALS as u32);
const SUPPLY: u64 = 1_000_000 * UNIT;

const SEED: u64 = 42;
const FEE: u16 = 100;
const PROTOCOL_FEE: u16 = 50;

type SendResult = Result<(), String>;

struct Pool {
    surfnet: Surfnet,
    rpc: RpcClient,
    authority: Keypair,
    trader: Keypair,
    mint_x: Pubkey,
    mint_y: Pubkey,
    config: Pubkey,
    mint_lp: Pubkey,
    vault_x: Pubkey,
    vault_y: Pubkey,
    treasury_x: Pubkey,
    treasury_y: Pubkey,
    authority_x: Pubkey,
    authority_y: Pubkey,
    authority_lp: Pubkey,
    trader_x: Pubkey,
    trader_y: Pubkey,
}

/// Packed SPL mint account: 82 bytes, initialised, no freeze authority.
fn mint_data(authority: &Pubkey) -> Vec<u8> {
    let mut data = vec![0u8; 82];
    data[..4].copy_from_slice(&1u32.to_le_bytes());
    data[4..36].copy_from_slice(authority.as_ref());
    data[44] = DECIMALS;
    data[45] = 1;
    data
}

impl Pool {
    /// Starts a fresh Surfnet with the program deployed, two funded users and the pool initialised.
    async fn new() -> Self {
        let authority = Keypair::new();
        let trader = Keypair::new();

        let surfnet = Surfnet::builder()
            .airdrop_addresses(vec![authority.pubkey(), trader.pubkey()])
            .start()
            .await
            .expect("surfnet should start");
        let rpc = surfnet.rpc_client();
        let cheats = surfnet.cheatcodes();

        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/amm.so");
        cheats
            .deploy(DeployProgram::new(amm::ID).so_path(so_path))
            .expect("program should deploy, run `anchor build` first");

        let mut mints = [Pubkey::new_unique(), Pubkey::new_unique()];
        mints.sort();
        let [mint_x, mint_y] = mints;
        for mint in mints {
            cheats
                .set_account(
                    &mint,
                    10_000_000,
                    &mint_data(&authority.pubkey()),
                    &token::ID,
                )
                .expect("mint should be created");
        }

        for owner in [authority.pubkey(), trader.pubkey()] {
            for mint in mints {
                cheats
                    .fund_token(&owner, &mint, SUPPLY, None)
                    .expect("user should be funded");
            }
        }

        let config = Pubkey::find_program_address(&[b"config", &SEED.to_le_bytes()], &amm::ID).0;
        let mint_lp = Pubkey::find_program_address(&[b"lp", config.as_ref()], &amm::ID).0;
        let treasury_x =
            Pubkey::find_program_address(&[b"treasury_x", config.as_ref()], &amm::ID).0;
        let treasury_y =
            Pubkey::find_program_address(&[b"treasury_y", config.as_ref()], &amm::ID).0;

        let pool = Self {
            vault_x: cheats.get_ata(&config, &mint_x, None),
            vault_y: cheats.get_ata(&config, &mint_y, None),
            authority_x: cheats.get_ata(&authority.pubkey(), &mint_x, None),
            authority_y: cheats.get_ata(&authority.pubkey(), &mint_y, None),
            authority_lp: cheats.get_ata(&authority.pubkey(), &mint_lp, None),
            trader_x: cheats.get_ata(&trader.pubkey(), &mint_x, None),
            trader_y: cheats.get_ata(&trader.pubkey(), &mint_y, None),
            surfnet,
            rpc,
            authority,
            trader,
            mint_x,
            mint_y,
            config,
            mint_lp,
            treasury_x,
            treasury_y,
        };

        pool.initialize(SEED, FEE, PROTOCOL_FEE)
            .expect("pool should initialise");
        pool
    }

    /// A pool that already holds 100 X and 400 Y, minted as 400 LP to the authority.
    async fn seeded() -> Self {
        let pool = Self::new().await;
        pool.deposit_as_authority(400 * UNIT, 100 * UNIT, 400 * UNIT)
            .expect("first deposit should succeed");
        pool
    }

    fn send(
        &self,
        ix: anchor_lang::solana_program::instruction::Instruction,
        signer: &Keypair,
    ) -> SendResult {
        let blockhash = self.rpc.get_latest_blockhash().map_err(|e| e.to_string())?;
        let message = Message::new(&[ix], Some(&signer.pubkey()));
        let mut tx = Transaction::new_unsigned(message);
        tx.try_sign(&[signer], blockhash)
            .map_err(|e| e.to_string())?;
        self.rpc
            .send_and_confirm_transaction(&tx)
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
    }

    fn initialize(&self, seed: u64, fee: u16, protocol_fee: u16) -> SendResult {
        let config = Pubkey::find_program_address(&[b"config", &seed.to_le_bytes()], &amm::ID).0;
        let cheats = self.surfnet.cheatcodes();

        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: amm::ID,
            accounts: accounts::Initialize {
                authority: self.authority.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                config,
                mint_lp: Pubkey::find_program_address(&[b"lp", config.as_ref()], &amm::ID).0,
                vault_x: cheats.get_ata(&config, &self.mint_x, None),
                vault_y: cheats.get_ata(&config, &self.mint_y, None),
                treasury_x: Pubkey::find_program_address(
                    &[b"treasury_x", config.as_ref()],
                    &amm::ID,
                )
                .0,
                treasury_y: Pubkey::find_program_address(
                    &[b"treasury_y", config.as_ref()],
                    &amm::ID,
                )
                .0,
                associated_token_program: associated_token::ID,
                token_program: token::ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: instruction::Initialize {
                seed,
                fee,
                protocol_fee,
            }
            .data(),
        };
        self.send(ix, &self.authority)
    }

    fn deposit_as_authority(&self, lp: u64, max_x: u64, max_y: u64) -> SendResult {
        self.deposit(
            &self.authority,
            self.authority_x,
            self.authority_y,
            self.authority_lp,
            lp,
            max_x,
            max_y,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn deposit(
        &self,
        user: &Keypair,
        user_x: Pubkey,
        user_y: Pubkey,
        user_lp: Pubkey,
        amount: u64,
        max_x: u64,
        max_y: u64,
    ) -> SendResult {
        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: amm::ID,
            accounts: accounts::Deposit {
                user: user.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                config: self.config,
                mint_lp: self.mint_lp,
                vault_x: self.vault_x,
                vault_y: self.vault_y,
                user_x,
                user_y,
                user_lp,
                associated_token_program: associated_token::ID,
                token_program: token::ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: instruction::Deposit {
                amount,
                max_x,
                max_y,
            }
            .data(),
        };
        self.send(ix, user)
    }

    fn withdraw(&self, amount: u64, min_x: u64, min_y: u64) -> SendResult {
        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: amm::ID,
            accounts: accounts::Withdraw {
                user: self.authority.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                config: self.config,
                mint_lp: self.mint_lp,
                vault_x: self.vault_x,
                vault_y: self.vault_y,
                user_x: self.authority_x,
                user_y: self.authority_y,
                user_lp: self.authority_lp,
                associated_token_program: associated_token::ID,
                token_program: token::ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: instruction::Withdraw {
                amount,
                min_x,
                min_y,
            }
            .data(),
        };
        self.send(ix, &self.authority)
    }

    fn swap(&self, is_x: bool, amount: u64, min: u64) -> SendResult {
        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: amm::ID,
            accounts: accounts::Swap {
                user: self.trader.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                config: self.config,
                mint_lp: self.mint_lp,
                vault_x: self.vault_x,
                vault_y: self.vault_y,
                treasury_x: self.treasury_x,
                treasury_y: self.treasury_y,
                user_x: self.trader_x,
                user_y: self.trader_y,
                associated_token_program: associated_token::ID,
                token_program: token::ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: instruction::Swap { is_x, amount, min }.data(),
        };
        self.send(ix, &self.trader)
    }

    fn claim(&self, signer: &Keypair, authority_x: Pubkey, authority_y: Pubkey) -> SendResult {
        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: amm::ID,
            accounts: accounts::Claim {
                authority: signer.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                config: self.config,
                treasury_x: self.treasury_x,
                treasury_y: self.treasury_y,
                authority_x,
                authority_y,
                associated_token_program: associated_token::ID,
                token_program: token::ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: instruction::Claim {}.data(),
        };
        self.send(ix, signer)
    }

    fn set_locked(&self, locked: bool) -> SendResult {
        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: amm::ID,
            accounts: accounts::SetLocked {
                authority: self.authority.pubkey(),
                config: self.config,
            }
            .to_account_metas(None),
            data: instruction::SetLocked { locked }.data(),
        };
        self.send(ix, &self.authority)
    }

    fn config_state(&self) -> Config {
        let account = self
            .rpc
            .get_account(&self.config)
            .expect("config should exist");
        Config::try_deserialize(&mut account.data.as_slice()).expect("config should deserialize")
    }

    /// Token account balance (amount lives at bytes 64..72 of the account data).
    fn balance(&self, address: &Pubkey) -> u64 {
        let account = self
            .rpc
            .get_account(address)
            .expect("token account should exist");
        u64::from_le_bytes(account.data[64..72].try_into().unwrap())
    }

    /// Mint supply (bytes 36..44 of the mint data).
    fn lp_supply(&self) -> u64 {
        let account = self
            .rpc
            .get_account(&self.mint_lp)
            .expect("lp mint should exist");
        u64::from_le_bytes(account.data[36..44].try_into().unwrap())
    }
}

/// Asserts that a failed send carries the given custom program error code.
#[track_caller]
fn assert_custom_error(result: SendResult, code: u32) {
    let err = result.expect_err("transaction should have failed");
    assert!(
        err.contains(&format!("Custom({code})")),
        "expected custom error {code}, got: {err}"
    );
}

fn amm_error(error: AmmError) -> u32 {
    error.into()
}

#[tokio::test(flavor = "multi_thread")]
async fn initializes_the_pool_with_its_fee_and_treasury_accounts() {
    let pool = Pool::new().await;
    let state = pool.config_state();

    assert_eq!(state.seed, SEED);
    assert_eq!(state.authority, pool.authority.pubkey());
    assert_eq!(state.mint_x, pool.mint_x);
    assert_eq!(state.mint_y, pool.mint_y);
    assert_eq!(state.fee, FEE);
    assert_eq!(state.protocol_fee, PROTOCOL_FEE);
    assert!(!state.locked);
    assert_eq!(pool.balance(&pool.treasury_x), 0);
    assert_eq!(pool.balance(&pool.treasury_y), 0);
    assert_eq!(pool.lp_supply(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_pool_whose_fee_is_out_of_range() {
    let pool = Pool::new().await;

    assert_custom_error(
        pool.initialize(SEED + 1, 10_000, PROTOCOL_FEE),
        amm_error(AmmError::InvalidFee),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn seeds_the_empty_pool_with_the_first_deposit() {
    let pool = Pool::new().await;
    let (x, y, lp) = (100 * UNIT, 400 * UNIT, 400 * UNIT);

    pool.deposit_as_authority(lp, x, y).unwrap();

    assert_eq!(pool.balance(&pool.vault_x), x);
    assert_eq!(pool.balance(&pool.vault_y), y);
    assert_eq!(pool.balance(&pool.authority_x), SUPPLY - x);
    assert_eq!(pool.balance(&pool.authority_y), SUPPLY - y);
    assert_eq!(pool.balance(&pool.authority_lp), lp);
    assert_eq!(pool.lp_supply(), lp);
}

#[tokio::test(flavor = "multi_thread")]
async fn adds_proportional_liquidity_on_a_later_deposit() {
    let pool = Pool::seeded().await;
    let lp = 100 * UNIT;

    let vault_x = pool.balance(&pool.vault_x);
    let vault_y = pool.balance(&pool.vault_y);
    let lp_before = pool.balance(&pool.authority_lp);
    let ratio_before = vault_y as f64 / vault_x as f64;

    pool.deposit_as_authority(lp, 50 * UNIT, 200 * UNIT)
        .unwrap();

    let ratio_after = pool.balance(&pool.vault_y) as f64 / pool.balance(&pool.vault_x) as f64;

    assert!(pool.balance(&pool.vault_x) > vault_x);
    assert!(pool.balance(&pool.vault_y) > vault_y);
    assert_eq!(pool.balance(&pool.authority_lp), lp_before + lp);
    assert!((ratio_after - ratio_before).abs() < 0.01);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_deposit_that_would_exceed_the_maximum_amounts() {
    let pool = Pool::seeded().await;

    assert_custom_error(
        pool.deposit_as_authority(100 * UNIT, 1, 1),
        amm_error(AmmError::SlippageExceeded),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn swaps_x_for_y_pays_the_treasury_and_keeps_the_invariant_growing() {
    let pool = Pool::seeded().await;
    let amount = 10 * UNIT;
    let expected_protocol = amount * PROTOCOL_FEE as u64 / 10_000;

    let vault_x = pool.balance(&pool.vault_x);
    let vault_y = pool.balance(&pool.vault_y);
    let trader_x = pool.balance(&pool.trader_x);
    let trader_y = pool.balance(&pool.trader_y);
    let k_before = vault_x as u128 * vault_y as u128;

    pool.swap(true, amount, 1).unwrap();

    let received = pool.balance(&pool.trader_y) - trader_y;
    let k_after = pool.balance(&pool.vault_x) as u128 * pool.balance(&pool.vault_y) as u128;

    assert_eq!(pool.balance(&pool.treasury_x), expected_protocol);
    assert_eq!(pool.balance(&pool.trader_x), trader_x - amount);
    assert!(received > 0);
    assert_eq!(pool.balance(&pool.vault_y), vault_y - received);
    assert!(pool.balance(&pool.vault_x) > vault_x);
    assert!(k_after >= k_before);
}

#[tokio::test(flavor = "multi_thread")]
async fn swaps_y_for_x_and_routes_the_protocol_fee_to_the_y_treasury() {
    let pool = Pool::seeded().await;
    let amount = 20 * UNIT;
    let expected_protocol = amount * PROTOCOL_FEE as u64 / 10_000;

    let vault_x = pool.balance(&pool.vault_x);
    let trader_x = pool.balance(&pool.trader_x);

    pool.swap(false, amount, 1).unwrap();

    assert_eq!(pool.balance(&pool.treasury_y), expected_protocol);
    assert!(pool.balance(&pool.trader_x) > trader_x);
    assert!(pool.balance(&pool.vault_x) < vault_x);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_swap_whose_output_misses_the_minimum() {
    let pool = Pool::seeded().await;

    assert_custom_error(
        pool.swap(true, UNIT, 1_000_000 * UNIT),
        amm_error(AmmError::SlippageExceeded),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn lets_the_authority_claim_the_accumulated_treasury_fees() {
    let pool = Pool::seeded().await;
    pool.swap(true, 10 * UNIT, 1).unwrap();
    pool.swap(false, 20 * UNIT, 1).unwrap();

    let treasury_x = pool.balance(&pool.treasury_x);
    let treasury_y = pool.balance(&pool.treasury_y);
    let authority_x = pool.balance(&pool.authority_x);
    let authority_y = pool.balance(&pool.authority_y);

    assert!(treasury_x > 0);
    assert!(treasury_y > 0);

    pool.claim(&pool.authority, pool.authority_x, pool.authority_y)
        .unwrap();

    assert_eq!(pool.balance(&pool.treasury_x), 0);
    assert_eq!(pool.balance(&pool.treasury_y), 0);
    assert_eq!(pool.balance(&pool.authority_x), authority_x + treasury_x);
    assert_eq!(pool.balance(&pool.authority_y), authority_y + treasury_y);
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_claim_when_the_treasury_is_empty() {
    let pool = Pool::seeded().await;

    assert_custom_error(
        pool.claim(&pool.authority, pool.authority_x, pool.authority_y),
        amm_error(AmmError::NothingToClaim),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_claim_from_anyone_but_the_pool_authority() {
    let pool = Pool::seeded().await;
    pool.swap(true, 10 * UNIT, 1).unwrap();

    assert_custom_error(
        pool.claim(&pool.trader, pool.trader_x, pool.trader_y),
        ErrorCode::ConstraintHasOne as u32,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn burns_lp_tokens_and_returns_the_underlying_on_withdraw() {
    let pool = Pool::seeded().await;
    let lp = 50 * UNIT;

    let vault_x = pool.balance(&pool.vault_x);
    let vault_y = pool.balance(&pool.vault_y);
    let user_x = pool.balance(&pool.authority_x);
    let user_y = pool.balance(&pool.authority_y);
    let lp_before = pool.balance(&pool.authority_lp);
    let supply = pool.lp_supply();

    pool.withdraw(lp, 1, 1).unwrap();

    assert_eq!(pool.balance(&pool.authority_lp), lp_before - lp);
    assert_eq!(pool.lp_supply(), supply - lp);
    assert!(pool.balance(&pool.vault_x) < vault_x);
    assert!(pool.balance(&pool.vault_y) < vault_y);
    assert!(pool.balance(&pool.authority_x) > user_x);
    assert!(pool.balance(&pool.authority_y) > user_y);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_withdrawal_that_would_return_less_than_the_minimum() {
    let pool = Pool::seeded().await;

    assert_custom_error(
        pool.withdraw(UNIT, 1_000_000 * UNIT, 1),
        amm_error(AmmError::SlippageExceeded),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn locks_the_pool_and_refuses_swaps_until_it_is_unlocked() {
    let pool = Pool::seeded().await;

    pool.set_locked(true).unwrap();
    assert!(pool.config_state().locked);
    assert_custom_error(pool.swap(true, UNIT, 1), amm_error(AmmError::PoolLocked));

    pool.set_locked(false).unwrap();
    assert!(!pool.config_state().locked);
    pool.swap(true, UNIT, 1).unwrap();
}
