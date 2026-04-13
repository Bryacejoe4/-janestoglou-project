// src/monitor.rs
// Polls on-chain state for tracked tokens and emits PriceTick events.
// Also detects risk signals: price rising while holders flat = distribution.

use anyhow::Result;
use dashmap::DashMap;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};
use std::{str::FromStr, sync::Arc, time::Duration};
use tokio::sync::mpsc;

use crate::{
    config::BotConfig,
    dex::pumpfun,
    logic::PumpCurveState,
    strategy::gembot::StrategyEvent,
};

// ─────────────────────────────────────────────────────────────────────────────
//  Price entry stored per token
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PriceEntry {
    pub price_sol:    f64,
    pub last_updated: std::time::Instant,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Monitor
// ─────────────────────────────────────────────────────────────────────────────

pub struct Monitor {
    rpc:    RpcClient,
    #[allow(dead_code)]
    config: BotConfig,
    tx:     mpsc::Sender<StrategyEvent>,
    prices: Arc<DashMap<String, PriceEntry>>,
}

impl Monitor {
    pub fn new(config: BotConfig, tx: mpsc::Sender<StrategyEvent>) -> Self {
        let rpc = RpcClient::new_with_commitment(
            config.rpc_url.clone(),
            CommitmentConfig::confirmed(),
        );
        Self {
            rpc,
            config,
            tx,
            prices: Arc::new(DashMap::new()),
        }
    }

    /// Add a token to the watchlist.
    pub fn watch(&self, mint: &str) {
        tracing::info!("Monitor: watching {}", &mint[..8.min(mint.len())]);
        self.prices.insert(mint.to_string(), PriceEntry {
            price_sol:    0.0,
            last_updated: std::time::Instant::now(),
        });
    }

    /// Remove from watchlist (called after position is closed).
    pub fn unwatch(&self, mint: &str) {
        self.prices.remove(mint);
    }

    /// Run the monitoring loop (poll every ~500 ms).
    pub async fn run(self) -> Result<()> {
        tracing::info!("Market monitor running…");
        let interval = Duration::from_millis(500);

        loop {
            let mints: Vec<String> = self.prices.iter().map(|e| e.key().clone()).collect();

            for mint_str in mints {
                match self.fetch_price(&mint_str).await {
                    Ok(price) => {
                        // Check for risk signals before emitting
                        self.check_risk_signals(&mint_str, price);

                        if let Some(mut entry) = self.prices.get_mut(&mint_str) {
                            entry.price_sol    = price;
                            entry.last_updated = std::time::Instant::now();
                        }

                        let _ = self.tx.try_send(StrategyEvent::PriceTick {
                            mint:      mint_str.clone(),
                            price_sol: price,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Price fetch failed for {}: {}", &mint_str[..8.min(mint_str.len())], e);
                    }
                }
            }

            tokio::time::sleep(interval).await;
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Price fetching
    // ─────────────────────────────────────────────────────────────────────────

    /// Read the Pump.fun bonding curve account and derive the current price.
    async fn fetch_price(&self, mint_str: &str) -> Result<f64> {
        let mint = Pubkey::from_str(mint_str)?;
        let curve_address = pumpfun::bonding_curve_pda(&mint);

        let account_data = self.rpc.get_account_data(&curve_address).await
            .map_err(|e| anyhow::anyhow!("get_account_data: {}", e))?;

        let state = parse_bonding_curve_account(&account_data)?;
        Ok(state.price_per_token())
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Risk signals
    // ─────────────────────────────────────────────────────────────────────────

    fn check_risk_signals(&self, mint_str: &str, current_price: f64) {
        if let Some(entry) = self.prices.get(mint_str) {
            let prev = entry.price_sol;
            if prev == 0.0 { return; }

            let change = (current_price - prev) / prev;

            // Warn on large sudden drops
            if change < -0.20 {
                tracing::warn!(
                    "⚠️  RISK: {} dropped {:.1}% in last tick",
                    &mint_str[..8.min(mint_str.len())],
                    change * 100.0
                );
            }

            // Warn on very rapid pump (possible distribution setup)
            if change > 0.30 {
                tracing::warn!(
                    "⚠️  RISK: {} pumped {:.1}% in last tick – possible distribution",
                    &mint_str[..8.min(mint_str.len())],
                    change * 100.0
                );
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Bonding curve account parser
// ─────────────────────────────────────────────────────────────────────────────

/// Deserialise the Pump.fun bonding-curve account data.
/// Layout (after 8-byte Anchor discriminator):
///   virtual_sol_reserves   u64
///   virtual_token_reserves u64
///   real_sol_reserves      u64
///   real_token_reserves    u64
///   token_total_supply     u64
///   complete               bool
fn parse_bonding_curve_account(data: &[u8]) -> Result<PumpCurveState> {
    // 8 byte discriminator + 5 * u64 (40 bytes) + 1 bool = 49 bytes minimum
    if data.len() < 49 {
        return Err(anyhow::anyhow!("Account data too short for bonding curve: {} bytes", data.len()));
    }

    let d = &data[8..]; // skip discriminator
    let read_u64 = |offset: usize| -> u64 {
        u64::from_le_bytes(d[offset..offset + 8].try_into().unwrap())
    };

    Ok(PumpCurveState {
        virtual_sol_reserves:   read_u64(0),
        virtual_token_reserves: read_u64(8),
        real_sol_reserves:      read_u64(16),
        real_token_reserves:    read_u64(24),
        token_total_supply:     read_u64(32),
        complete:               d[40] != 0,
    })
}
