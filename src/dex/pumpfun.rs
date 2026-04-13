// src/dex/pumpfun.rs
// Pump.fun buy / sell instruction builders (Anchor ABI).

use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program, sysvar,
};
use std::str::FromStr;

pub const PUMP_PROGRAM_ID:  &str = "6EF8rrecthR5DkZ8NThExdS1m596J1YAn8mSpf8NDfL8";
pub const FEE_RECIPIENT:    &str = "CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM";
pub const EVENT_AUTHORITY:  &str = "Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7jxXpXhH";

// Anchor discriminators (first 8 bytes of sha256("global:<ix_name>"))
const BUY_DISCRIMINATOR:  [u8; 8] = [102,   6,  61,  18,   1, 218, 235, 234];
const SELL_DISCRIMINATOR: [u8; 8] = [ 51, 230, 133, 164,   1, 127, 131, 173];

pub fn program_id() -> Pubkey {
    Pubkey::from_str(PUMP_PROGRAM_ID).unwrap()
}

// ─────────────────────────────────────────────────────────────────────────────
//  PDA helpers
// ─────────────────────────────────────────────────────────────────────────────

pub fn global_pda() -> Pubkey {
    let pid = program_id();
    Pubkey::find_program_address(&[b"global"], &pid).0
}

pub fn bonding_curve_pda(mint: &Pubkey) -> Pubkey {
    let pid = program_id();
    Pubkey::find_program_address(&[b"bonding-curve", mint.as_ref()], &pid).0
}

pub fn associated_bonding_curve(mint: &Pubkey) -> Pubkey {
    let bc = bonding_curve_pda(mint);
    crate::utils::get_ata(&bc, mint)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Buy instruction
// ─────────────────────────────────────────────────────────────────────────────

/// Build a Pump.fun `buy` instruction.
///
/// * `token_amount`  – tokens to buy (raw units with 6 decimals)
/// * `max_sol_cost`  – maximum lamports to spend (includes slippage buffer)
pub fn build_buy_instruction(
    payer:         &Pubkey,
    mint:          &Pubkey,
    token_amount:  u64,
    max_sol_cost:  u64,
) -> Instruction {
    let program_id           = program_id();
    let global               = global_pda();
    let bonding_curve        = bonding_curve_pda(mint);
    let assoc_bonding_curve  = associated_bonding_curve(mint);
    let user_ata             = crate::utils::get_ata(payer, mint);
    let fee_recipient        = Pubkey::from_str(FEE_RECIPIENT).unwrap();
    let event_authority      = Pubkey::from_str(EVENT_AUTHORITY).unwrap();

    let mut data = BUY_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&token_amount.to_le_bytes());
    data.extend_from_slice(&max_sol_cost.to_le_bytes());

    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(global,              false),
            AccountMeta::new(fee_recipient,                false),
            AccountMeta::new_readonly(*mint,               false),
            AccountMeta::new(bonding_curve,                false),
            AccountMeta::new(assoc_bonding_curve,          false),
            AccountMeta::new(user_ata,                     false),
            AccountMeta::new(*payer,                       true),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(spl_token_program(), false),
            AccountMeta::new_readonly(sysvar::rent::id(), false),
            AccountMeta::new_readonly(event_authority,     false),
            AccountMeta::new_readonly(program_id,          false),
        ],
        data,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Sell instruction
// ─────────────────────────────────────────────────────────────────────────────

/// Build a Pump.fun `sell` instruction.
///
/// * `token_amount`   – tokens to sell
/// * `min_sol_output` – minimum lamports to accept (0 = instant dump, no protection)
pub fn build_sell_instruction(
    payer:          &Pubkey,
    mint:           &Pubkey,
    token_amount:   u64,
    min_sol_output: u64,
) -> Instruction {
    let program_id           = program_id();
    let global               = global_pda();
    let bonding_curve        = bonding_curve_pda(mint);
    let assoc_bonding_curve  = associated_bonding_curve(mint);
    let user_ata             = crate::utils::get_ata(payer, mint);
    let fee_recipient        = Pubkey::from_str(FEE_RECIPIENT).unwrap();
    let event_authority      = Pubkey::from_str(EVENT_AUTHORITY).unwrap();

    let mut data = SELL_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&token_amount.to_le_bytes());
    data.extend_from_slice(&min_sol_output.to_le_bytes());

    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(global,              false),
            AccountMeta::new(fee_recipient,                false),
            AccountMeta::new_readonly(*mint,               false),
            AccountMeta::new(bonding_curve,                false),
            AccountMeta::new(assoc_bonding_curve,          false),
            AccountMeta::new(user_ata,                     false),
            AccountMeta::new(*payer,                       true),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(spl_token_program(), false),
            AccountMeta::new_readonly(sysvar::rent::id(), false),
            AccountMeta::new_readonly(event_authority,     false),
            AccountMeta::new_readonly(program_id,          false),
        ],
        data,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Internal helper
// ─────────────────────────────────────────────────────────────────────────────

/// Token program ID bridged to SDK Pubkey.
fn spl_token_program() -> Pubkey {
    Pubkey::from_str(&spl_token::id().to_string()).unwrap()
}
