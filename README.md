# AMM — Constant-Product Automated Market Maker on Solana

A fully on-chain, constant-product AMM (Automated Market Maker) built with the [Anchor](https://www.anchor-lang.com/) framework on Solana. It supports dual-fee token swaps, proportional liquidity provisioning, LP token issuance/burning, protocol fee collection with authority-only claims, and an emergency pool-lock mechanism.

---

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [On-Chain State](#on-chain-state)
- [Instructions](#instructions)
- [Error Codes](#error-codes)
- [Fee Model](#fee-model)
- [Account PDAs](#account-pdas)
- [Project Structure](#project-structure)
- [Prerequisites](#prerequisites)
- [Build & Deploy](#build--deploy)
- [Running Tests](#running-tests)
- [Test Results](#test-results)
- [Test Coverage Summary](#test-coverage-summary)
- [Dependencies](#dependencies)

---

## Overview

This program implements an **x·y = k constant-product curve** for two arbitrary SPL token mints (`mint_x` and `mint_y`). The key design decisions are:

| Feature | Detail |
|---|---|
| Curve | Constant product (`x * y = k`) |
| LP Token decimals | 6 |
| Swap fee | Configurable per-pool, in basis points (0–9 999) |
| Protocol fee | Separate configurable cut, in basis points (0–9 999) |
| Treasury | Per-pool PDAs that accumulate protocol fees independently for each token |
| Pool lock | Authority can pause/resume swaps and deposits at any time |
| Test runtime | [Surfpool](https://surfpool.dev/) (embedded validator with cheatcode support) |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        AMM Program                          │
│                                                             │
│  ┌──────────┐  ┌──────────┐  ┌────────┐  ┌─────────────┐  │
│  │initialize│  │ deposit  │  │withdraw│  │    swap     │  │
│  └──────────┘  └──────────┘  └────────┘  └─────────────┘  │
│                                                             │
│  ┌──────────────────┐  ┌────────────────────────────────┐  │
│  │      claim       │  │         set_locked             │  │
│  └──────────────────┘  └────────────────────────────────┘  │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                  Config PDA (State)                  │   │
│  │  seed · authority · mint_x · mint_y · fee ·         │   │
│  │  protocol_fee · locked · config_bump · lp_bump      │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
│  Vaults: vault_x, vault_y (ATAs owned by config PDA)       │
│  Treasuries: treasury_x, treasury_y (PDAs, protocol fees)  │
│  LP Mint: mint_lp (PDA, authority = config)                 │
└─────────────────────────────────────────────────────────────┘
```

The `Config` account is the central state object, keyed by a user-supplied `seed` so multiple independent pools can coexist on the same program.

---

## On-Chain State

### `Config` (`programs/amm/src/state.rs`)

```rust
pub struct Config {
    pub seed: u64,           // Unique pool identifier
    pub authority: Pubkey,   // Pool authority (can claim fees & lock/unlock)
    pub mint_x: Pubkey,      // Token X mint
    pub mint_y: Pubkey,      // Token Y mint
    pub fee: u16,            // LP swap fee in basis points (e.g. 100 = 1%)
    pub protocol_fee: u16,   // Protocol fee in basis points (e.g. 50 = 0.5%)
    pub locked: bool,        // If true, swaps & deposits are paused
    pub config_bump: u8,     // PDA bump for config account
    pub lp_bump: u8,         // PDA bump for LP mint
}
```

**Account size:** `8 (discriminator) + Config::INIT_SPACE`

---

## Instructions

### 1. `initialize`

Creates a new AMM pool for a given token pair.

**Parameters:**
| Param | Type | Description |
|---|---|---|
| `seed` | `u64` | Unique identifier for this pool |
| `fee` | `u16` | LP fee in basis points (must be < 10 000) |
| `protocol_fee` | `u16` | Protocol fee in basis points (must be < 10 000) |

**Accounts created:**
- `Config` PDA
- `mint_lp` — LP token mint (PDA, 6 decimals, authority = config)
- `vault_x`, `vault_y` — ATAs for the pool's token reserves
- `treasury_x`, `treasury_y` — PDA token accounts for protocol fee accumulation

**Validation:**
- `fee < 10_000` and `protocol_fee < 10_000`
- `mint_x ≠ mint_y`

---

### 2. `deposit`

Adds liquidity to the pool and mints LP tokens to the caller.

**Parameters:**
| Param | Type | Description |
|---|---|---|
| `amount` | `u64` | LP tokens to mint |
| `max_x` | `u64` | Maximum token X to deposit (slippage guard) |
| `max_y` | `u64` | Maximum token Y to deposit (slippage guard) |

**Behavior:**
- **First deposit (empty pool):** Seeds the pool with exactly `max_x` of X and `max_y` of Y.
- **Subsequent deposits:** Uses the constant-product curve (`ConstantProduct::xy_deposit_amounts_from_l`) to calculate the proportional X and Y amounts for the requested `amount` of LP tokens.
- Transfers X and Y from the user's ATAs into the pool vaults.
- Mints `amount` LP tokens to the user.

**Validation:**
- Pool must not be locked
- `amount > 0`, both computed token amounts must be `> 0`
- Computed amounts must satisfy `x ≤ max_x` and `y ≤ max_y`

---

### 3. `withdraw`

Burns LP tokens and returns the proportional share of pool reserves to the caller.

**Parameters:**
| Param | Type | Description |
|---|---|---|
| `amount` | `u64` | LP tokens to burn |
| `min_x` | `u64` | Minimum token X to receive (slippage guard) |
| `min_y` | `u64` | Minimum token Y to receive (slippage guard) |

**Behavior:**
- Calculates the proportional X and Y to return using `ConstantProduct::xy_withdraw_amounts_from_l`.
- Burns the LP tokens from the user's LP ATA.
- Transfers X and Y from the vaults to the user.

**Validation:**
- Pool must not be locked
- `amount > 0`, LP supply must be `> 0`
- Returned amounts must satisfy `x ≥ min_x` and `y ≥ min_y`

---

### 4. `swap`

Swaps one token for the other through the pool.

**Parameters:**
| Param | Type | Description |
|---|---|---|
| `is_x` | `bool` | `true` = sell X for Y; `false` = sell Y for X |
| `amount` | `u64` | Amount of input token to sell |
| `min` | `u64` | Minimum output token to receive (slippage guard) |

**Behavior:**
1. Deducts the protocol fee from `amount` first: `protocol_amount = amount * protocol_fee / 10_000`.
2. The remaining `swap_amount` is passed to the constant-product curve.
3. `protocol_amount` of the input token is transferred directly to the appropriate treasury.
4. The curve-computed output is transferred from the vault to the user.

**Validation:**
- Pool must not be locked
- `amount > 0`, both vault reserves must be `> 0`
- Output must be `≥ min`

---

### 5. `claim`

Transfers all accumulated protocol fees from both treasuries to the pool authority's token accounts.

**Validation:**
- Signer must match `config.authority`
- At least one treasury must have a non-zero balance

---

### 6. `set_locked`

Toggles the pool's emergency lock.

**Parameters:**
| Param | Type | Description |
|---|---|---|
| `locked` | `bool` | `true` = pause; `false` = resume |

**Validation:**
- Signer must match `config.authority`

---

## Error Codes

| Error | Message |
|---|---|
| `InvalidAmount` | Amount must be greater than zero |
| `InvalidFee` | Fee cannot exceed 10000 basis points |
| `IdenticalMints` | Both mints must be different |
| `PoolLocked` | The pool is locked |
| `SlippageExceeded` | Slippage limit exceeded |
| `NoLiquidity` | The pool holds no liquidity |
| `NothingToClaim` | There are no fees to claim |
| `Overflow` | Curve arithmetic overflowed |
| `Underflow` | Curve arithmetic underflowed |
| `CurveFailure` | The curve rejected the operation |

---

## Fee Model

For every swap of input amount `A`:

```
protocol_amount = A × protocol_fee / 10_000   (→ treasury)
swap_amount     = A − protocol_amount          (→ curve)
lp_fee          = swap_amount × fee / 10_000  (→ remains in vault as LP earnings)
output          = curve_output(swap_amount)
```

Both fees are additive — the LP fee stays inside the pool (growing the invariant k), while the protocol fee is extracted to a separate treasury account claimable only by the authority.

---

## Account PDAs

| Account | Seeds | Description |
|---|---|---|
| `config` | `[b"config", seed.to_le_bytes()]` | Pool state |
| `mint_lp` | `[b"lp", config.key()]` | LP token mint |
| `treasury_x` | `[b"treasury_x", config.key()]` | Protocol fee account for token X |
| `treasury_y` | `[b"treasury_y", config.key()]` | Protocol fee account for token Y |
| `vault_x` | ATA(config, mint_x) | Pool reserve for token X |
| `vault_y` | ATA(config, mint_y) | Pool reserve for token Y |

---

## Project Structure

```
amm/
├── Anchor.toml                      # Anchor workspace config
├── Cargo.toml                       # Workspace manifest
├── rust-toolchain.toml              # Pinned Rust toolchain
├── asset/
│   └── Screenshot 2026-10-12 ...   # Test results screenshot
├── programs/
│   └── amm/
│       ├── Cargo.toml
│       ├── src/
│       │   ├── lib.rs               # Program entry point & instruction dispatch
│       │   ├── state.rs             # Config account definition
│       │   ├── errors.rs            # AmmError enum
│       │   └── instructions/
│       │       ├── mod.rs
│       │       ├── initialize.rs    # Pool creation
│       │       ├── deposit.rs       # Liquidity provisioning
│       │       ├── withdraw.rs      # Liquidity removal
│       │       ├── swap.rs          # Token swap
│       │       ├── claim.rs         # Protocol fee claim
│       │       └── set_locked.rs    # Emergency lock toggle
│       └── tests/
│           └── surfpool.rs          # End-to-end integration tests
└── app/                             # (Frontend / client, if present)
```

---

## Prerequisites

| Tool | Version |
|---|---|
| Rust | See `rust-toolchain.toml` |
| Anchor CLI | ≥ 1.2.1 |
| Solana CLI | Compatible with Anchor version |
| Node.js + Yarn | For Anchor toolchain scripts |

Install the Anchor CLI:

```bash
cargo install --git https://github.com/coral-xyz/anchor avm --locked --force
avm install latest && avm use latest
```

---

## Build & Deploy

```bash
# Build the program
anchor build

# Deploy to localnet
anchor deploy

# Or run build + deploy + test in one step
anchor test
```

> **Note:** The test suite loads the compiled `.so` from `target/deploy/amm.so`. Always run `anchor build` before running tests standalone.

---

## Running Tests

The test suite uses [Surfpool](https://surfpool.dev/) — an embedded Solana validator that starts in-process, removing the need for a separate `solana-test-validator` process.

```bash
# Run all tests via Anchor (recommended)
anchor test

# Or run the Rust tests directly
cargo test --package amm
```

Tests are in `programs/amm/tests/surfpool.rs` and use `#[tokio::test(flavor = "multi_thread")]` since Surfpool requires a multi-threaded async runtime.

---

## Test Results

All **14 integration tests** pass. The screenshot below was captured from the most recent `anchor test` run:

![Test results — 14/14 passing](asset/Screenshot%202026-10-12%20at%2005.10.21.png)

**Terminal output:**

```
running 14 tests
test initializes_the_pool_with_its_fee_and_treasury_accounts ... ok
test rejects_a_deposit_that_would_exceed_the_maximum_amounts ... ok
test refuses_a_claim_when_the_treasury_is_empty ... ok
test rejects_a_pool_whose_fee_is_out_of_range ... ok
test refuses_a_claim_from_anyone_but_the_pool_authority ... ok
test burns_lp_tokens_and_returns_the_underlying_on_withdraw ... ok
test adds_proportional_liquidity_on_a_later_deposit ... ok
test rejects_a_withdrawal_that_would_return_less_than_the_minimum ... ok
test rejects_a_swap_whose_output_misses_the_minimum ... ok
test locks_the_pool_and_refuses_swaps_until_it_is_unlocked ... ok
test lets_the_authority_claim_the_accumulated_treasury_fees ... ok
test seeds_the_empty_pool_with_the_first_deposit ... ok
test swaps_y_for_x_and_routes_the_protocol_fee_to_the_y_treasury ... ok
test swaps_x_for_y_pays_the_treasury_and_keeps_the_invariant_growing ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.23s
```

---

## Test Coverage Summary

| # | Test Name | Instruction Tested | What It Verifies |
|---|---|---|---|
| 1 | `initializes_the_pool_with_its_fee_and_treasury_accounts` | `initialize` | Config state is set correctly; treasuries and LP mint start at zero |
| 2 | `rejects_a_pool_whose_fee_is_out_of_range` | `initialize` | Fee ≥ 10 000 bps triggers `InvalidFee` |
| 3 | `seeds_the_empty_pool_with_the_first_deposit` | `deposit` | First deposit seeds vaults with exact `max_x`/`max_y`; LP tokens minted |
| 4 | `adds_proportional_liquidity_on_a_later_deposit` | `deposit` | Subsequent deposit preserves the x/y ratio to within 1% |
| 5 | `rejects_a_deposit_that_would_exceed_the_maximum_amounts` | `deposit` | Tight slippage limits trigger `SlippageExceeded` |
| 6 | `swaps_x_for_y_pays_the_treasury_and_keeps_the_invariant_growing` | `swap` | X→Y swap: correct protocol fee to `treasury_x`; k non-decreasing |
| 7 | `swaps_y_for_x_and_routes_the_protocol_fee_to_the_y_treasury` | `swap` | Y→X swap: correct protocol fee to `treasury_y` |
| 8 | `rejects_a_swap_whose_output_misses_the_minimum` | `swap` | `min` guard triggers `SlippageExceeded` |
| 9 | `lets_the_authority_claim_the_accumulated_treasury_fees` | `claim` | Both treasuries drained to zero; authority balances increase accordingly |
| 10 | `refuses_a_claim_when_the_treasury_is_empty` | `claim` | Empty treasury triggers `NothingToClaim` |
| 11 | `refuses_a_claim_from_anyone_but_the_pool_authority` | `claim` | Non-authority signer triggers Anchor `ConstraintHasOne` error |
| 12 | `burns_lp_tokens_and_returns_the_underlying_on_withdraw` | `withdraw` | LP tokens burned; vaults decrease; user token balances increase |
| 13 | `rejects_a_withdrawal_that_would_return_less_than_the_minimum` | `withdraw` | Tight `min_x`/`min_y` triggers `SlippageExceeded` |
| 14 | `locks_the_pool_and_refuses_swaps_until_it_is_unlocked` | `set_locked` | Lock prevents swaps; unlock re-enables them |

---

## Dependencies

### Runtime

| Crate | Version | Purpose |
|---|---|---|
| `anchor-lang` | 1.2.1 | Anchor framework (with `init-if-needed` feature) |
| `anchor-spl` | 1.2.1 | SPL Token & Associated Token CPI helpers |
| `constant-product-curve` | git | Constant-product math (deposit, withdraw, swap) |

### Dev / Test

| Crate | Version | Purpose |
|---|---|---|
| `surfpool-sdk` | 1.6.1 | Embedded Solana validator with cheatcodes |
| `tokio` | 1.43.0 | Async runtime for tests (`rt-multi-thread`) |
| `solana-message` | 4.3 | Building unsigned transactions in tests |
| `solana-transaction` | 4.1 | Signing and sending transactions in tests |

---

## Program ID

```
F3te5tv5fryVxPj3k4X6D7kRbitvPsvFFxq65NtKPn2Z
```

(Localnet — update in `Anchor.toml` and `declare_id!` when deploying to devnet/mainnet.)

---

## License

MIT
