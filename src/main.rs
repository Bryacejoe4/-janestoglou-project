// src/main.rs
pub mod config;
pub mod wallet;
pub mod utils;
pub mod logic;
pub mod engine;
pub mod dex;
pub mod strategy;
pub mod sniper;
pub mod monitor;
pub mod copy_trade;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;
use std::str::FromStr;
use solana_sdk::signature::Signer;

use config::BotConfig;
use engine::TradingEngine;
use wallet::WalletManager;
use strategy::gembot::{GembotStrategy, StrategyEvent};

// ─────────────────────────────────────────────────────────────────────────────
//  CLI definition
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name    = "botx",
    version = "0.2.0",
    about   = "Solana HFT Bot – Gembot-strategy automated trading",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the full automated bot (sniper + Gembot strategy + copy-trade)
    Run,

    /// Manually buy a token on Pump.fun
    Buy {
        /// Token mint address
        mint: String,
        /// SOL amount in lamports to spend (default 0.1 SOL)
        #[arg(long, default_value_t = 100_000_000)]
        lamports: u64,
    },

    /// Manually sell ALL tokens for a given mint
    Sell {
        /// Token mint address
        mint: String,
        /// Minimum SOL output in lamports (0 = instant sell, no slippage protection)
        #[arg(long, default_value_t = 0)]
        min_sol: u64,
    },

    /// Show wallet balance(s)
    Balance,

    /// Run offline sanity-check tests (no real transactions)
    Test,

    /// Scan Pump.fun activity without trading (read-only)
    Scan,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Entry point
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env before anything else
    dotenv::dotenv().ok();

    // Structured logging  (set RUST_LOG=botx=debug for verbose output)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("botx=info,warn")),
        )
        .with_target(false)
        .with_level(true)
        .init();

    let cli = Cli::parse();
    let cfg = BotConfig::load()?;

    match cli.command {
        // ── Balance ──────────────────────────────────────────────────────────
        Command::Balance => {
            let wallets = WalletManager::from_env()?;
            let engine  = TradingEngine::new(cfg);

            for i in 0..wallets.len() {
                let kp  = wallets.get(i).unwrap();
                let bal = engine.sol_balance(&kp.pubkey()).await?;
                println!(
                    "Wallet {} | {} | {:.6} SOL",
                    i + 1,
                    kp.pubkey(),
                    utils::lamports_to_sol(bal),
                );
            }
        }

        // ── Manual Buy ───────────────────────────────────────────────────────
        Command::Buy { mint, lamports } => {
            let wallets  = WalletManager::from_env()?;
            let engine   = TradingEngine::new(cfg.clone());
            let mint_pk  = solana_sdk::pubkey::Pubkey::from_str(&mint)
                .map_err(|_| anyhow::anyhow!("Invalid mint address: {}", mint))?;

            // Conservative token estimate – real sniper reads bonding curve
            // Use 1 token unit as placeholder so the instruction is well-formed;
            // set max_sol_cost high so the AMM fills whatever it can.
            let tokens    = 1_000_000u64; // 1 token (6 dec) – Pump.fun will give correct amount
            let max_cost  = logic::max_sol_cost_with_slippage(lamports, cfg.strategy.slippage_bps);

            let sig = engine.pump_buy(wallets.main(), &mint_pk, tokens, max_cost).await?;
            println!("BUY submitted: https://solscan.io/tx/{}", sig);
        }

        // ── Manual Sell ──────────────────────────────────────────────────────
        Command::Sell { mint, min_sol } => {
            let wallets = WalletManager::from_env()?;
            let engine  = TradingEngine::new(cfg);
            let mint_pk = solana_sdk::pubkey::Pubkey::from_str(&mint)
                .map_err(|_| anyhow::anyhow!("Invalid mint address: {}", mint))?;

            let balance = engine.token_balance(&wallets.main().pubkey(), &mint_pk).await?;
            if balance == 0 {
                println!("No tokens to sell for {}", mint);
                return Ok(());
            }
            println!("Selling {} tokens…", balance);
            let sig = engine.pump_sell(wallets.main(), &mint_pk, balance, min_sol).await?;
            println!("SELL submitted: https://solscan.io/tx/{}", sig);
        }

        // ── Full Automated Run ───────────────────────────────────────────────
        Command::Run => {
            let wallets = WalletManager::from_env()?;
            let engine  = TradingEngine::new(cfg.clone());

            // Show starting balance
            let start_bal = engine.sol_balance(&wallets.main().pubkey()).await?;
            tracing::info!(
                "🚀 Bot starting | Wallet: {} | Balance: {:.4} SOL",
                wallets.main().pubkey(),
                utils::lamports_to_sol(start_bal),
            );

            // MPSC channel: all event producers → strategy consumer
            let (tx, rx) = mpsc::channel::<StrategyEvent>(1024);

            // ── Strategy task ────────────────────────────────────────────────
            let strategy = GembotStrategy::new(&cfg, engine, wallets);
            let strat_handle = tokio::spawn(strategy.run(rx));

            // ── Sniper task ──────────────────────────────────────────────────
            if cfg.sniper.enabled {
                let sn  = sniper::Sniper::new(cfg.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Err(e) = sn.run().await {
                        tracing::error!("Sniper crashed: {}", e);
                    }
                });
            } else {
                tracing::info!("Sniper disabled (sniper.enabled = false)");
            }

            // ── Copy-trade task ──────────────────────────────────────────────
            if cfg.copy_trade.enabled {
                let ct = copy_trade::CopyTrader::new(cfg.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Err(e) = ct.run().await {
                        tracing::error!("Copy-trader crashed: {}", e);
                    }
                });
            }

            // ── Graceful shutdown on Ctrl+C ──────────────────────────────────
            println!("✅ Bot is running. Press Ctrl+C to stop.");
            tokio::signal::ctrl_c().await?;
            tracing::info!("Shutdown signal received…");
            let _ = tx.send(StrategyEvent::Shutdown).await;
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                strat_handle,
            ).await;
            println!("Bot stopped cleanly.");
        }

        // ── Offline Test ─────────────────────────────────────────────────────
        Command::Test => {
            run_offline_tests();
        }

        // ── Scan (read-only) ─────────────────────────────────────────────────
        Command::Scan => {
            tracing::info!("📡 Scanning Pump.fun activity (read-only)…");
            let engine = TradingEngine::new(cfg.clone());
            use solana_sdk::pubkey::Pubkey;

            let pump_program = Pubkey::from_str(dex::pumpfun::PUMP_PROGRAM_ID)?;

            loop {
                match engine.rpc.get_signatures_for_address(&pump_program).await {
                    Ok(sigs) => {
                        for sig in sigs.into_iter().take(5) {
                            println!("🔎 https://solscan.io/tx/{}", sig.signature);
                        }
                    }
                    Err(e) => tracing::warn!("get_signatures: {}", e),
                }
                println!("⏳ Polling… (Ctrl+C to stop)");
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
//  Offline sanity tests
// ─────────────────────────────────────────────────────────────────────────────

fn run_offline_tests() {
    use logic::{Logic, PoolReserves, PumpCurveState, min_amount_out_after_slippage, max_sol_cost_with_slippage};
    use strategy::filters::{FilterConfig, FilterVerdict, TokenFilter, TokenSnapshot};

    println!("\n══════════════════════════════════════════════");
    println!("  solana-hft-botx  ·  Offline Test Suite");
    println!("══════════════════════════════════════════════\n");

    // ── Test 1: Constant-product AMM quote ───────────────────────────────────
    let reserves = PoolReserves { reserve_in: 1_000_000_000, reserve_out: 150_000_000_000 };
    let quote    = Logic::calculate_amm_quote(100_000_000, &reserves, 100)
        .expect("AMM quote failed");
    println!("✅ [1/6] AMM Quote");
    println!("   Input:    0.1 SOL (100_000_000 lamports)");
    println!("   Expected: {} USDC units", quote.expected_amount_out);
    println!("   Min out:  {} USDC units (1% slippage)", quote.min_amount_out);
    println!("   Impact:   {} bps\n", quote.price_impact_bps);
    assert!(quote.expected_amount_out > 0);
    assert!(quote.min_amount_out < quote.expected_amount_out);

    // ── Test 2: Pump.fun bonding curve ───────────────────────────────────────
    let curve = PumpCurveState::default();
    let buy_p = curve.buy_price(1_000_000).expect("buy_price failed"); // 1 token
    let tok   = curve.tokens_for_sol(100_000_000).expect("tokens_for_sol failed");
    println!("✅ [2/6] Pump.fun Bonding Curve");
    println!("   Cost for 1 token:          {} lamports", buy_p);
    println!("   Tokens for 0.1 SOL:        {}", tok);
    println!("   Starting price (SOL/token): {:.9}\n", curve.price_per_token());

    // ── Test 3: Slippage helpers ─────────────────────────────────────────────
    let min_out  = min_amount_out_after_slippage(1_000_000, 100); // 1% slippage
    let max_cost = max_sol_cost_with_slippage(1_000_000, 100);
    println!("✅ [3/6] Slippage Helpers");
    println!("   min_amount_out (1% slip on 1_000_000): {}", min_out);
    println!("   max_sol_cost   (1% slip on 1_000_000): {}\n", max_cost);
    assert_eq!(min_out,  990_000);
    assert_eq!(max_cost, 1_010_000);

    // ── Test 4: Token filter – pass ───────────────────────────────────────────
    let good = TokenSnapshot {
        mint:              "So11111111111111111111111111111111111111112".into(),
        volume_usd_5m:     35_000.0,
        liquidity_sol:     10.0,
        holder_count:      150,
        top10_pct:         0.28,
        fresh_wallet_pct:  0.15,
        sniper_bundle_pct: 0.08,
        age_seconds:       60,
        organic_chart:     true,
        price_sol:         0.0001,
        ..Default::default()
    };
    let filter  = TokenFilter::new(FilterConfig::default());
    let verdict = filter.evaluate(&good);
    println!("✅ [4/6] Token Filter – should PASS");
    println!("   Verdict: {:?}\n", verdict);
    assert_eq!(verdict, FilterVerdict::Pass);

    // ── Test 5: Token filter – reject ─────────────────────────────────────────
    let mut bad = good.clone();
    bad.sniper_bundle_pct = 0.60;
    let bad_verdict = filter.evaluate(&bad);
    println!("✅ [5/6] Token Filter – should REJECT (high snipers)");
    println!("   Verdict: {:?}\n", bad_verdict);
    assert!(matches!(bad_verdict, FilterVerdict::Reject(_)));

    // ── Test 6: Risk manager ──────────────────────────────────────────────────
    use strategy::risk::RiskManager;
    use config::RiskConfig;
    let risk_cfg = RiskConfig {
        max_position_pct:     0.15,
        daily_loss_limit_pct: 0.10,
        max_sol_per_trade:    500_000_000,
    };
    let mut rm  = RiskManager::new(risk_cfg, 2_000_000_000); // 2 SOL starting
    let allowed = rm.allowed_trade_size(2_000_000_000).unwrap();
    println!("✅ [6/6] Risk Manager");
    println!("   Starting balance: 2.0 SOL");
    println!("   Allowed trade:    {:.4} SOL (min(15% of 2 SOL, cap))", utils::lamports_to_sol(allowed));
    rm.record_trade(-250_000_000); // -0.25 SOL loss  (12.5 % → over 10 % limit)
    println!("   After -0.25 SOL loss, paused: {}\n", rm.is_paused());
    assert!(rm.is_paused());

    println!("══════════════════════════════════════════════");
    println!("  ✅ ALL 6 TESTS PASSED – Foundation is solid");
    println!("══════════════════════════════════════════════\n");
}
