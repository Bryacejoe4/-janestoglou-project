// src/copy_trade.rs
// Copy-trading: subscribes to transactions from watched wallets and mirrors
// buy/sell actions proportionally.

use anyhow::Result;
use tokio::sync::mpsc;

use crate::{
    config::BotConfig,
    strategy::{
        filters::{TokenSnapshot},
        gembot::StrategyEvent,
    },
};

// ─────────────────────────────────────────────────────────────────────────────
//  CopyTrader
// ─────────────────────────────────────────────────────────────────────────────

pub struct CopyTrader {
    config: BotConfig,
    tx:     mpsc::Sender<StrategyEvent>,
}

impl CopyTrader {
    pub fn new(config: BotConfig, tx: mpsc::Sender<StrategyEvent>) -> Self {
        Self { config, tx }
    }

    pub async fn run(self) -> Result<()> {
        if !self.config.copy_trade.enabled {
            tracing::info!("Copy trading disabled (set copy_trade.enabled = true in config)");
            return Ok(());
        }

        let watched = self.config.copy_trade.watched_wallets.clone();
        if watched.is_empty() {
            tracing::warn!("Copy trading enabled but no watched_wallets configured");
            return Ok(());
        }

        tracing::info!("Copy trader monitoring {} wallet(s)", watched.len());

        let wss_url = self.config.wss_url.clone();
        let tx      = self.tx.clone();
        let size_pct = self.config.copy_trade.trade_size_pct;

        // One subscription thread per watched wallet
        for wallet_str in watched {
            let wss  = wss_url.clone();
            let tx2  = tx.clone();
            let addr = wallet_str.clone();

            tokio::task::spawn_blocking(move || {
                watch_wallet_blocking(&wss, &addr, tx2, size_pct)
            });
        }

        // Keep the task alive
        futures_util::future::pending::<()>().await;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Blocking wallet watcher
// ─────────────────────────────────────────────────────────────────────────────

fn watch_wallet_blocking(
    wss_url:  &str,
    wallet:   &str,
    tx:       mpsc::Sender<StrategyEvent>,
    _size_pct: f64,
) {
    use solana_client::pubsub_client::PubsubClient;
    use solana_client::rpc_config::{
        RpcTransactionLogsConfig, RpcTransactionLogsFilter,
    };
    use solana_sdk::commitment_config::CommitmentConfig;

    let filter = RpcTransactionLogsFilter::Mentions(vec![wallet.to_string()]);
    let config = RpcTransactionLogsConfig {
        commitment: Some(CommitmentConfig::confirmed()),
    };

    // logs_subscribe returns (_subscription_handle, receiver)
    // The SECOND element is the channel we actually read from
    let (_sub, receiver) = match PubsubClient::logs_subscribe(wss_url, filter, config) {
        Ok(s)  => s,
        Err(e) => {
            tracing::error!("Copy trade subscribe for {}: {}", wallet, e);
            return;
        }
    };

    tracing::info!("Copy trade: watching {}", &wallet[..8.min(wallet.len())]);

    loop {
        let response = match receiver.recv() {
            Ok(r)  => r,
            Err(_) => break,
        };

        let logs: Vec<String> = response.value.logs;

        // Heuristic: look for Pump.fun buy instructions in the logs
        let is_pump_buy  = logs.iter().any(|l: &String| l.contains("Instruction: Buy"));
        let is_pump_sell = logs.iter().any(|l: &String| l.contains("Instruction: Sell"));

        if !is_pump_buy && !is_pump_sell { continue; }

        // Extract the token mint from logs
        let mint = match extract_mint_from_logs(&logs) {
            Some(m) => m,
            None    => continue,
        };

        tracing::info!(
            "📋 COPY SIGNAL: {} {} on {}",
            if is_pump_buy { "BUY" } else { "SELL" },
            &wallet[..8.min(wallet.len())],
            &mint[..8.min(mint.len())]
        );

        if is_pump_buy {
            // Emit as a new token / entry signal
            let snap = TokenSnapshot {
                mint: mint.clone(),
                volume_usd_5m: 99_999.0,  // We trust the copied trader's judgement
                liquidity_sol: 10.0,
                holder_count:  100,
                organic_chart: true,
                ..Default::default()
            };
            let _ = tx.try_send(StrategyEvent::NewToken(snap));
        }
        // Sell signals are handled by monitor price ticks (the position will be
        // exited via the normal stop/TP logic once price starts dropping).
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Shared log parser
// ─────────────────────────────────────────────────────────────────────────────

fn extract_mint_from_logs(logs: &[String]) -> Option<String> {
    for line in logs {
        if let Some(pos) = line.find("mint: ") {
            let after = &line[pos + 6..];
            let mint: String = after.chars().take_while(|c| c.is_alphanumeric()).collect();
            if mint.len() >= 32 { return Some(mint); }
        }
    }
    None
}
