// src/config.rs
// Loads from config/default.toml, then overlays .env values.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

// ─────────────────────────────────────────────────────────────────────────────
//  Top-level config
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BotConfig {
    pub rpc_url:   String,
    pub wss_url:   String,
    pub strategy:  StrategyConfig,
    pub risk:      RiskConfig,
    pub sniper:    SniperConfig,
    pub copy_trade: CopyTradeConfig,
    pub jito:      JitoConfig,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Sub-configs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StrategyConfig {
    pub entry_split:              bool,
    pub second_entry_dip_pct:     f64,
    pub max_entries_per_token:    u8,
    pub take_profit_pct:          f64,
    pub stop_loss_pct:            f64,
    pub moonbag_pct:              f64,
    pub trailing_stop_pct:        f64,
    pub slippage_bps:             u16,
    pub priority_fee_micro_lamports: u64,
    pub max_retries:              u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RiskConfig {
    pub max_position_pct:      f64,
    pub daily_loss_limit_pct:  f64,
    pub max_sol_per_trade:     u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SniperConfig {
    pub enabled:                bool,
    pub min_liquidity_lamports: u64,
    pub max_fresh_wallet_pct:   f64,
    pub max_sniper_bundle_pct:  f64,
    pub buy_amount_lamports:    u64,
    pub min_volume_usd:         f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CopyTradeConfig {
    pub enabled:          bool,
    pub watched_wallets:  Vec<String>,
    pub trade_size_pct:   f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JitoConfig {
    pub enabled:      bool,
    pub tip_lamports: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Internal TOML mirror (all fields optional so we can layer env-vars on top)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct TomlStrategy {
    entry_split:                Option<bool>,
    second_entry_dip_pct:       Option<f64>,
    max_entries_per_token:      Option<u8>,
    take_profit_pct:            Option<f64>,
    stop_loss_pct:              Option<f64>,
    moonbag_pct:                Option<f64>,
    trailing_stop_pct:          Option<f64>,
    slippage_bps:               Option<u16>,
    priority_fee_micro_lamports: Option<u64>,
    max_retries:                Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
struct TomlRisk {
    max_position_pct:     Option<f64>,
    daily_loss_limit_pct: Option<f64>,
    max_sol_per_trade:    Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct TomlSniper {
    enabled:                Option<bool>,
    min_liquidity_lamports: Option<u64>,
    max_fresh_wallet_pct:   Option<f64>,
    max_sniper_bundle_pct:  Option<f64>,
    buy_amount_lamports:    Option<u64>,
    min_volume_usd:         Option<f64>,
}

#[derive(Debug, Deserialize, Default)]
struct TomlCopyTrade {
    enabled:         Option<bool>,
    watched_wallets: Option<Vec<String>>,
    trade_size_pct:  Option<f64>,
}

#[derive(Debug, Deserialize, Default)]
struct TomlJito {
    enabled:      Option<bool>,
    tip_lamports: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct TomlRoot {
    strategy:   Option<TomlStrategy>,
    risk:       Option<TomlRisk>,
    sniper:     Option<TomlSniper>,
    copy_trade: Option<TomlCopyTrade>,
    jito:       Option<TomlJito>,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Loader
// ─────────────────────────────────────────────────────────────────────────────

impl BotConfig {
    /// Load config/default.toml then overlay environment variables.
    pub fn load() -> Result<Self> {
        // Load TOML (optional – use defaults if file missing)
        let toml_root: TomlRoot = match fs::read_to_string("config/default.toml") {
            Ok(contents) => toml::from_str(&contents)
                .context("Failed to parse config/default.toml")?,
            Err(_) => TomlRoot::default(),
        };

        let ts = toml_root.strategy.unwrap_or_default();
        let tr = toml_root.risk.unwrap_or_default();
        let tsn = toml_root.sniper.unwrap_or_default();
        let tc = toml_root.copy_trade.unwrap_or_default();
        let tj = toml_root.jito.unwrap_or_default();

        // ENV overrides (env values beat TOML values)
        let slippage_bps: u16 = std::env::var("SLIPPAGE_BPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(ts.slippage_bps)
            .unwrap_or(200);

        let jito_tip: u64 = std::env::var("JITO_TIP_LAMPORTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(tj.tip_lamports)
            .unwrap_or(15_000_000);

        let max_sol: u64 = std::env::var("MAX_SOL_PER_TRADE")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(tr.max_sol_per_trade)
            .unwrap_or(500_000_000);

        Ok(BotConfig {
            rpc_url: std::env::var("RPC_URL").context("RPC_URL missing in .env")?,
            wss_url: std::env::var("WSS_URL").unwrap_or_else(|_| {
                // Best-effort: derive WSS from HTTP URL
                std::env::var("RPC_URL")
                    .unwrap_or_default()
                    .replace("https://", "wss://")
                    .replace("http://", "ws://")
            }),
            strategy: StrategyConfig {
                entry_split:                  ts.entry_split.unwrap_or(true),
                second_entry_dip_pct:         ts.second_entry_dip_pct.unwrap_or(0.25),
                max_entries_per_token:        ts.max_entries_per_token.unwrap_or(2),
                take_profit_pct:              ts.take_profit_pct.unwrap_or(0.25),
                stop_loss_pct:                ts.stop_loss_pct.unwrap_or(0.12),
                moonbag_pct:                  ts.moonbag_pct.unwrap_or(0.10),
                trailing_stop_pct:            ts.trailing_stop_pct.unwrap_or(0.08),
                slippage_bps,
                priority_fee_micro_lamports:  ts.priority_fee_micro_lamports.unwrap_or(100_000),
                max_retries:                  ts.max_retries.unwrap_or(3),
            },
            risk: RiskConfig {
                max_position_pct:     tr.max_position_pct.unwrap_or(0.15),
                daily_loss_limit_pct: tr.daily_loss_limit_pct.unwrap_or(0.10),
                max_sol_per_trade:    max_sol,
            },
            sniper: SniperConfig {
                enabled:                tsn.enabled.unwrap_or(true),
                min_liquidity_lamports: tsn.min_liquidity_lamports.unwrap_or(5_000_000_000),
                max_fresh_wallet_pct:   tsn.max_fresh_wallet_pct.unwrap_or(0.30),
                max_sniper_bundle_pct:  tsn.max_sniper_bundle_pct.unwrap_or(0.15),
                buy_amount_lamports:    tsn.buy_amount_lamports.unwrap_or(100_000_000),
                min_volume_usd:         tsn.min_volume_usd.unwrap_or(30_000.0),
            },
            copy_trade: CopyTradeConfig {
                enabled:         tc.enabled.unwrap_or(false),
                watched_wallets: tc.watched_wallets.unwrap_or_default(),
                trade_size_pct:  tc.trade_size_pct.unwrap_or(0.50),
            },
            jito: JitoConfig {
                enabled:      tj.enabled.unwrap_or(true),
                tip_lamports: jito_tip,
            },
        })
    }
}
