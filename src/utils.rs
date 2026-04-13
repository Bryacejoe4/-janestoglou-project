// src/utils.rs

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// ─────────────────────────────────────────────────────────────────────────────
//  ATA derivation
//  Bridges the version mismatch between solana-sdk Pubkey and spl's Pubkey.
// ─────────────────────────────────────────────────────────────────────────────

pub fn get_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    // Round-trip through strings to resolve the solana-sdk vs spl version split
    let spl_wallet = to_spl_pubkey(wallet);
    let spl_mint   = to_spl_pubkey(mint);
    let ata = spl_associated_token_account::get_associated_token_address(&spl_wallet, &spl_mint);
    from_spl_pubkey(&ata)
}

/// solana_sdk::Pubkey → spl_token::solana_program::Pubkey
pub fn to_spl_pubkey(pk: &Pubkey) -> spl_token::solana_program::pubkey::Pubkey {
    spl_token::solana_program::pubkey::Pubkey::from_str(&pk.to_string())
        .expect("pubkey round-trip failed")
}

/// spl_token::solana_program::Pubkey → solana_sdk::Pubkey
pub fn from_spl_pubkey(pk: &spl_token::solana_program::pubkey::Pubkey) -> Pubkey {
    Pubkey::from_str(&pk.to_string()).expect("pubkey round-trip failed")
}

// ─────────────────────────────────────────────────────────────────────────────
//  Formatting helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Convert lamports to SOL for display.
pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / 1_000_000_000.0
}

/// Convert SOL to lamports.
pub fn sol_to_lamports(sol: f64) -> u64 {
    (sol * 1_000_000_000.0) as u64
}

/// Shorten a pubkey for log output.
pub fn short_key(pk: &Pubkey) -> String {
    let s = pk.to_string();
    format!("{}…{}", &s[..4], &s[s.len() - 4..])
}

// ─────────────────────────────────────────────────────────────────────────────
//  Retry helper
// ─────────────────────────────────────────────────────────────────────────────

/// Retry an async closure up to `max_attempts` times with exponential back-off.
pub async fn retry_async<F, Fut, T>(max_attempts: u32, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_err = anyhow::anyhow!("retry_async called with 0 attempts");
    for attempt in 1..=max_attempts {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_err = e;
                if attempt < max_attempts {
                    let backoff = 200u64 * (1 << (attempt - 1).min(4));
                    tracing::warn!("Attempt {}/{} failed, retrying in {}ms…", attempt, max_attempts, backoff);
                    tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                }
            }
        }
    }
    Err(last_err)
}
