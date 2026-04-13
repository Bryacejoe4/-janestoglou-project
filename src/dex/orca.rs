// src/dex/orca.rs
// Orca Whirlpool swap instruction builder.

use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};
use std::str::FromStr;

pub const WHIRLPOOL_PROGRAM: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

// Anchor discriminator for Whirlpool `swap`
const SWAP_DISCRIMINATOR: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];

pub fn program_id() -> Pubkey {
    WHIRLPOOL_PROGRAM.parse().expect("const")
}

// ─────────────────────────────────────────────────────────────────────────────
//  Account layout required by a Whirlpool swap
// ─────────────────────────────────────────────────────────────────────────────

pub struct OrcaSwapAccounts {
    pub whirlpool:    Pubkey,
    pub token_vault_a: Pubkey,
    pub token_vault_b: Pubkey,
    pub tick_array_0: Pubkey,
    pub tick_array_1: Pubkey,
    pub tick_array_2: Pubkey,
    pub oracle:       Pubkey,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Instruction builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a raw Whirlpool swap instruction.
///
/// Argument layout (Anchor ABI):
///   discriminator (8) | amount (u64) | other_amount_threshold (u64) |
///   sqrt_price_limit (u128) | amount_specified_is_input (bool) | a_to_b (bool)
pub fn build_swap_instruction(
    accounts:                  &OrcaSwapAccounts,
    user_owner:                &Pubkey,
    user_source_token:         &Pubkey,
    user_destination_token:    &Pubkey,
    amount:                    u64,
    other_amount_threshold:    u64,
    sqrt_price_limit:          u128,
    amount_specified_is_input: bool,
    a_to_b:                    bool,
) -> Instruction {
    let program_id = program_id();
    let token_pid  = sdk_pubkey_from_str(&spl_token::id().to_string());

    let mut data = Vec::with_capacity(8 + 8 + 8 + 16 + 1 + 1);
    data.extend_from_slice(&SWAP_DISCRIMINATOR);
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(&other_amount_threshold.to_le_bytes());
    data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
    data.push(u8::from(amount_specified_is_input));
    data.push(u8::from(a_to_b));

    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(token_pid,                     false),
            AccountMeta::new_readonly(*user_owner,                   true),
            AccountMeta::new(accounts.whirlpool,                     false),
            AccountMeta::new(*user_source_token,                     false),
            AccountMeta::new(accounts.token_vault_a,                 false),
            AccountMeta::new(*user_destination_token,                false),
            AccountMeta::new(accounts.token_vault_b,                 false),
            AccountMeta::new(accounts.tick_array_0,                  false),
            AccountMeta::new(accounts.tick_array_1,                  false),
            AccountMeta::new(accounts.tick_array_2,                  false),
            AccountMeta::new_readonly(accounts.oracle,               false),
        ],
        data,
    }
}

fn sdk_pubkey_from_str(s: &str) -> Pubkey {
    Pubkey::from_str(s).expect("pubkey parse failed")
}
