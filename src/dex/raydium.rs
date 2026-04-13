// src/dex/raydium.rs
// Raydium AMM V4 swap instruction builder.

use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};
#[allow(unused_imports)]
use std::str::FromStr;
pub const AMM_V4_PROGRAM: &str  = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";
pub const SERUM_PROGRAM:  &str  = "srmqPvymJeFKQ4zdt99No696YSeB68Y57zbyC5vM7F";
pub const TOKEN_PROGRAM:  &str  = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

pub fn program_id() -> Pubkey {
    AMM_V4_PROGRAM.parse().expect("const")
}

// ─────────────────────────────────────────────────────────────────────────────
//  Pool account layout
// ─────────────────────────────────────────────────────────────────────────────

pub struct RaydiumPoolAccounts {
    pub amm_id:                  Pubkey,
    pub amm_authority:           Pubkey,
    pub amm_open_orders:         Pubkey,
    pub amm_target_orders:       Pubkey,
    pub pool_coin_token_account: Pubkey, // base vault
    pub pool_pc_token_account:   Pubkey, // quote vault (SOL / USDC)
    pub serum_market:            Pubkey,
    pub serum_bids:              Pubkey,
    pub serum_asks:              Pubkey,
    pub serum_event_queue:       Pubkey,
    pub serum_coin_vault:        Pubkey,
    pub serum_pc_vault:          Pubkey,
    pub serum_vault_signer:      Pubkey,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Instruction builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a Raydium AMM V4 `SwapBaseIn` instruction.
///
/// Data layout: [9u8 (index)] ++ amount_in (u64 LE) ++ min_amount_out (u64 LE)
pub fn build_swap_instruction(
    accounts:       &RaydiumPoolAccounts,
    user_owner:     &Pubkey,
    user_source:    &Pubkey,
    user_dest:      &Pubkey,
    amount_in:      u64,
    min_amount_out: u64,
) -> Instruction {
    let program_id = program_id();
    let token_pid  = TOKEN_PROGRAM.parse::<Pubkey>().expect("const");
    let serum_pid  = SERUM_PROGRAM.parse::<Pubkey>().expect("const");

    let mut data = Vec::with_capacity(17);
    data.push(9u8); // SwapBaseIn discriminator
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_amount_out.to_le_bytes());

    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(token_pid,                              false),
            AccountMeta::new(accounts.amm_id,                                false),
            AccountMeta::new_readonly(accounts.amm_authority,                false),
            AccountMeta::new(accounts.amm_open_orders,                       false),
            AccountMeta::new(accounts.amm_target_orders,                     false),
            AccountMeta::new(accounts.pool_coin_token_account,               false),
            AccountMeta::new(accounts.pool_pc_token_account,                 false),
            AccountMeta::new_readonly(serum_pid,                             false),
            AccountMeta::new(accounts.serum_market,                          false),
            AccountMeta::new(accounts.serum_bids,                            false),
            AccountMeta::new(accounts.serum_asks,                            false),
            AccountMeta::new(accounts.serum_event_queue,                     false),
            AccountMeta::new(accounts.serum_coin_vault,                      false),
            AccountMeta::new(accounts.serum_pc_vault,                        false),
            AccountMeta::new_readonly(accounts.serum_vault_signer,           false),
            AccountMeta::new(copy_pubkey(user_source),                       false),
            AccountMeta::new(copy_pubkey(user_dest),                         false),
            AccountMeta::new_readonly(copy_pubkey(user_owner),               true),
        ],
        data,
    }
}

fn copy_pubkey(pk: &Pubkey) -> Pubkey {
    Pubkey::new_from_array(pk.to_bytes())
}
