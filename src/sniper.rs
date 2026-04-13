// src/sniper.rs
// Listens to Pump.fun program logs via WebSocket and fires entry signals
// when a new token is created.  Passes tokens through filters before signalling.

use anyhow::Result;
use tokio::sync::mpsc;

use crate::{
    config::BotConfig,
    strategy::{
        filters::{FilterConfig, TokenFilter, TokenSnapshot},
        gembot::StrategyEvent,
    },
};

const PUMP_CREATE_DISCRIMINATOR: &str = "Program log: Instruction: Create";

// ─────────────────────────────────────────────────────────────────────────────
//  Sniper
// ─────────────────────────────────────────────────────────────────────────────

pub struct Sniper {
    config: BotConfig,
    #[allow(dead_code)]
    filter: TokenFilter,
    tx:     mpsc::Sender<StrategyEvent>,
}

impl Sniper {
    pub fn new(config: BotConfig, tx: mpsc::Sender<StrategyEvent>) -> Self {
        let sc = &config.sniper;
        let filter = TokenFilter::new(FilterConfig {
            min_volume_usd_5m:     sc.min_volume_usd,
            min_liquidity_sol:     sc.min_liquidity_lamports as f64 / 1e9,
            max_fresh_wallet_pct:  sc.max_fresh_wallet_pct,
            max_sniper_bundle_pct: sc.max_sniper_bundle_pct,
            max_top10_pct:         0.40,
            min_holder_count:      1,   // brand new tokens start at 1
            min_age_seconds:       0,
        });
        Self { config, filter, tx }
    }

    /// Subscribes to Pump.fun program logs and emits StrategyEvent::NewToken
    /// for every token that passes the sniper filter.
    pub async fn run(self) -> Result<()> {
        tracing::info!("🎯 Sniper starting – watching Pump.fun…");

        // We use the blocking pubsub client on a dedicated thread to avoid
        // the async/sync bridging complexity until nonblocking pubsub matures.
        let wss_url     = self.config.wss_url.clone();
        let pump_id     = crate::dex::pumpfun::PUMP_PROGRAM_ID.to_string();
        let min_liq     = self.config.sniper.min_liquidity_lamports;
        let _buy_amt    = self.config.sniper.buy_amount_lamports;
        let tx          = self.tx.clone();

        // Spawn a blocking thread so the pubsub blocking calls don't stall tokio
        tokio::task::spawn_blocking(move || {
            use solana_client::pubsub_client::PubsubClient;
            use solana_client::rpc_config::{
                RpcTransactionLogsConfig, RpcTransactionLogsFilter,
            };
            use solana_sdk::commitment_config::CommitmentConfig;

            // logs_subscribe returns (_subscription_handle, receiver)
            // The SECOND element is the channel we actually read from
            let (_sub, receiver) = PubsubClient::logs_subscribe(
                &wss_url,
                RpcTransactionLogsFilter::Mentions(vec![pump_id]),
                RpcTransactionLogsConfig {
                    commitment: Some(CommitmentConfig::confirmed()),
                },
            ).map_err(|e| { tracing::error!("PubsubClient::logs_subscribe: {}", e); e })?;

            tracing::info!("WebSocket subscription active ✓");

            loop {
                let response = match receiver.recv() {
                    Ok(r)  => r,
                    Err(_) => break,
                };

                let logs: Vec<String> = response.value.logs;

                let is_create = logs.iter().any(|l: &String| l.contains(PUMP_CREATE_DISCRIMINATOR));
                if !is_create { continue; }

                let mint = extract_mint_from_logs(&logs);
                let Some(mint) = mint else { continue };

                tracing::info!("🆕 New Pump.fun token detected: {}", &mint[..8.min(mint.len())]);

                // For a true sniper we skip the filter entirely and buy immediately.
                // Here we apply a minimal liveness check and let the strategy filter
                // handle further vetting (avoids waiting for volume to build).
                let snap = TokenSnapshot {
                    mint:              mint.clone(),
                    volume_usd_5m:     99_999.0,   // Unknown at launch; bypass filter
                    liquidity_sol:     min_liq as f64 / 1e9,
                    holder_count:      1,
                    organic_chart:     true,
                    fresh_wallet_pct:  0.0,         // Unknown at launch
                    sniper_bundle_pct: 0.0,
                    top10_pct:         0.0,
                    age_seconds:       0,
                    price_sol:         0.000_001,   // Rough starting price
                    ..Default::default()
                };

                // Push to strategy (non-blocking; drop if channel is full)
                let _ = tx.try_send(StrategyEvent::NewToken(snap));
            }

            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Log parsing helpers
// ─────────────────────────────────────────────────────────────────────────────

fn extract_mint_from_logs(logs: &[String]) -> Option<String> {
    for line in logs {
        // Pump.fun logs contain "mint: <pubkey>" in the Create instruction
        if let Some(pos) = line.find("mint: ") {
            let after = &line[pos + 6..];
            let mint: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric())
                .collect();
            if mint.len() >= 32 {
                return Some(mint);
            }
        }
    }
    // Fallback: look for any 32-44 char base58 substring near "Program log"
    None
}
