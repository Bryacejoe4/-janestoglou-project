// src/wallet.rs
// Fixed: handles 32-byte seeds AND 64-byte full keypairs, multi-wallet support.

use anyhow::{anyhow, Result};
use solana_sdk::signature::{Keypair, SeedDerivable, Signer};

// ─────────────────────────────────────────────────────────────────────────────
//  WalletManager
// ─────────────────────────────────────────────────────────────────────────────

pub struct WalletManager {
    keypairs: Vec<Keypair>,
}

impl WalletManager {
    /// Load wallets from env.
    /// Checks PRIVATE_KEYS (comma-separated) first, then PRIVATE_KEY.
    pub fn from_env() -> Result<Self> {
        let raw = std::env::var("PRIVATE_KEYS")
            .ok()
            .or_else(|| std::env::var("PRIVATE_KEY").ok())
            .unwrap_or_default();

        let parts: Vec<&str> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();

        if parts.is_empty() {
            return Err(anyhow!("No private key found. Set PRIVATE_KEY or PRIVATE_KEYS in .env"));
        }

        let keypairs = parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                parse_flexible_key(p)
                    .map_err(|e| anyhow!("Wallet #{}: {}", i + 1, e))
            })
            .collect::<Result<Vec<_>>>()?;

        tracing::info!("Loaded {} wallet(s)", keypairs.len());
        for (i, kp) in keypairs.iter().enumerate() {
            tracing::info!("  Wallet {}: {}", i + 1, kp.pubkey());
        }

        Ok(Self { keypairs })
    }

    /// Primary trading wallet.
    pub fn main(&self) -> &Keypair {
        &self.keypairs[0]
    }

    /// Wallet at index, or None if out of range.
    pub fn get(&self, index: usize) -> Option<&Keypair> {
        self.keypairs.get(index)
    }

    /// Number of loaded wallets.
    pub fn len(&self) -> usize {
        self.keypairs.len()
    }

    /// All wallets (useful for multi-wallet strategies).
    pub fn all(&self) -> &[Keypair] {
        &self.keypairs
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Key parser
// ─────────────────────────────────────────────────────────────────────────────

/// Accepts:
///   • 64-byte base58 (Phantom / Solflare full-keypair export)
///   • 32-byte base58 (CLI generated seed)
fn parse_flexible_key(s: &str) -> Result<Keypair> {
    let bytes = bs58::decode(s)
        .into_vec()
        .map_err(|e| anyhow!("Base58 decode error: {}", e))?;

    match bytes.len() {
        64 => Keypair::from_bytes(&bytes)
            .map_err(|e| anyhow!("Invalid 64-byte keypair: {}", e)),

        32 => {
            // from_seed is available via the SeedDerivable trait re-export
            let seed: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow!("Could not convert to [u8;32]"))?;
            Ok(Keypair::from_seed(&seed)
                .map_err(|e| anyhow!("Invalid 32-byte seed: {}", e))?)
        }

        n => Err(anyhow!(
            "Wrong key length: {} bytes. Must be 32 (seed) or 64 (full keypair).",
            n
        )),
    }
}
