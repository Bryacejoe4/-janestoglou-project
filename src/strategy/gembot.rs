// src/strategy/gembot.rs
// Replicates the Gembot signal strategy programmatically.
//
// Entry:
//   • Token passes all filters (see filters.rs)
//   • Option A – buy 100 % immediately
//   • Option B – split entry: 50 % now, wait for ≥ 25 % dip, then 50 % more
//   • Max 2 entries per token
//
// Exit:
//   • Take-profit: sell (100 % - moonbag_pct) at target gain
//   • Trailing stop: sell entire position if price falls > trailing_pct from peak
//   • Hard stop-loss: sell everything if price drops > stop_loss_pct from entry
//   • Moonbag: remaining % held until manual close or further TP

use std::collections::HashMap;
use tokio::sync::mpsc;
use anyhow::Result;

use solana_sdk::signature::Signer;

use crate::{
    config::{BotConfig, StrategyConfig},
    engine::TradingEngine,
    logic::{Position, PositionEntry},
    strategy::filters::{FilterVerdict, TokenFilter, TokenSnapshot},
    wallet::WalletManager,
};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// ─────────────────────────────────────────────────────────────────────────────
//  Events that flow into the strategy
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum StrategyEvent {
    /// A new token was detected (from sniper or manual)
    NewToken(TokenSnapshot),
    /// Updated price tick for a token we are watching or holding
    PriceTick { mint: String, price_sol: f64 },
    /// Emergency stop
    Shutdown,
}

// ─────────────────────────────────────────────────────────────────────────────
//  GembotStrategy
// ─────────────────────────────────────────────────────────────────────────────

pub struct GembotStrategy {
    config:     StrategyConfig,
    engine:     TradingEngine,
    wallets:    WalletManager,
    filter:     TokenFilter,
    positions:  HashMap<String, Position>,
    // Tracks how many entries have been made per token
    entry_counts: HashMap<String, u8>,
}

impl GembotStrategy {
    pub fn new(bot_cfg: &BotConfig, engine: TradingEngine, wallets: WalletManager) -> Self {
        use crate::strategy::filters::{FilterConfig};
        let sc = &bot_cfg.sniper;
        let filter = TokenFilter::new(FilterConfig {
            min_volume_usd_5m:     sc.min_volume_usd,
            min_liquidity_sol:     sc.min_liquidity_lamports as f64 / 1e9,
            max_fresh_wallet_pct:  sc.max_fresh_wallet_pct,
            max_sniper_bundle_pct: sc.max_sniper_bundle_pct,
            max_top10_pct:         0.40,
            min_holder_count:      50,
            min_age_seconds:       30,
        });

        Self {
            config:       bot_cfg.strategy.clone(),
            engine,
            wallets,
            filter,
            positions:    HashMap::new(),
            entry_counts: HashMap::new(),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Main event loop (run in its own tokio task)
    // ─────────────────────────────────────────────────────────────────────────

    pub async fn run(mut self, mut rx: mpsc::Receiver<StrategyEvent>) {
        tracing::info!("Gembot strategy running…");
        while let Some(event) = rx.recv().await {
            match event {
                StrategyEvent::NewToken(snap) => {
                    if let Err(e) = self.handle_new_token(snap).await {
                        tracing::error!("handle_new_token: {}", e);
                    }
                }
                StrategyEvent::PriceTick { mint, price_sol } => {
                    if let Err(e) = self.handle_price_tick(&mint, price_sol).await {
                        tracing::error!("handle_price_tick: {}", e);
                    }
                }
                StrategyEvent::Shutdown => {
                    tracing::warn!("Strategy shutdown requested.");
                    break;
                }
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  New token handler
    // ─────────────────────────────────────────────────────────────────────────

    async fn handle_new_token(&mut self, snap: TokenSnapshot) -> Result<()> {
        let verdict = self.filter.evaluate(&snap);
        if let FilterVerdict::Reject(reason) = verdict {
            tracing::debug!("SKIP {} – {}", &snap.mint[..8.min(snap.mint.len())], reason);
            return Ok(());
        }

        let entries_so_far = *self.entry_counts.get(&snap.mint).unwrap_or(&0);
        if entries_so_far >= self.config.max_entries_per_token {
            tracing::debug!("SKIP {} – max entries reached", &snap.mint[..8.min(snap.mint.len())]);
            return Ok(());
        }

        tracing::info!("✅ ENTRY SIGNAL for {}", snap.mint);

        let keypair     = self.wallets.main();
        let mint        = Pubkey::from_str(&snap.mint)?;
        let sol_balance = self.engine.sol_balance(&keypair.pubkey()).await?;

        // Determine how much to buy
        let available = sol_balance.saturating_sub(5_000_000); // leave 0.005 SOL for fees
        let trade_sol = if self.config.entry_split {
            available / 2 // First 50 %
        } else {
            available
        };

        if trade_sol < 1_000_000 {
            tracing::warn!("Insufficient balance for trade");
            return Ok(());
        }

        // Estimate tokens to receive (simplified – real impl reads bonding curve)
        let tokens_estimate = (trade_sol as f64 / snap.price_sol / 1e3) as u64 * 1000;
        let max_sol_cost    = crate::logic::max_sol_cost_with_slippage(
            trade_sol, self.config.slippage_bps,
        );

        match self.engine.pump_buy(keypair, &mint, tokens_estimate, max_sol_cost).await {
            Ok(sig) => {
                tracing::info!("BUY executed: {}", sig);
                let position = self.positions
                    .entry(snap.mint.clone())
                    .or_insert_with(|| Position::new(&snap.mint));
                position.entries.push(PositionEntry {
                    sol_spent:     trade_sol,
                    tokens_bought: tokens_estimate,
                    entry_price:   snap.price_sol,
                });
                position.peak_price_sol = snap.price_sol;
                *self.entry_counts.entry(snap.mint.clone()).or_insert(0) += 1;
            }
            Err(e) => tracing::error!("BUY failed: {}", e),
        }
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Price tick handler
    // ─────────────────────────────────────────────────────────────────────────

    async fn handle_price_tick(&mut self, mint_str: &str, price_sol: f64) -> Result<()> {
        let Some(position) = self.positions.get_mut(mint_str) else {
            // Check if we should do a second entry (split strategy)
            return self.check_second_entry(mint_str, price_sol).await;
        };

        if position.is_closed { return Ok(()); }

        position.update_peak(price_sol);
        let multiplier = position.pnl_multiplier(price_sol);
        let _avg_cost  = position.avg_cost();

        // ── Take profit ────────────────────────────────────────────────────────
        if multiplier >= 1.0 + self.config.take_profit_pct {
            tracing::info!(
                "🎯 TAKE PROFIT on {} | gain={:.1}%",
                &mint_str[..8.min(mint_str.len())],
                (multiplier - 1.0) * 100.0
            );
            self.exit_position(mint_str, price_sol, false).await?;
        }

        // ── Trailing stop ──────────────────────────────────────────────────────
        else if position.trailing_stop_triggered(price_sol, self.config.trailing_stop_pct) {
            tracing::warn!(
                "📉 TRAILING STOP on {} | peak={:.6} current={:.6}",
                &mint_str[..8.min(mint_str.len())],
                position.peak_price_sol, price_sol
            );
            self.exit_position(mint_str, price_sol, true).await?;
        }

        // ── Hard stop loss ─────────────────────────────────────────────────────
        else if multiplier <= 1.0 - self.config.stop_loss_pct {
            tracing::warn!(
                "🛑 STOP LOSS on {} | loss={:.1}%",
                &mint_str[..8.min(mint_str.len())],
                (1.0 - multiplier) * 100.0
            );
            self.exit_position(mint_str, price_sol, true).await?;
        }

        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Second entry (split strategy: buy after -25 % dip)
    // ─────────────────────────────────────────────────────────────────────────

    async fn check_second_entry(&mut self, mint_str: &str, current_price: f64) -> Result<()> {
        if !self.config.entry_split { return Ok(()); }
        let entries = *self.entry_counts.get(mint_str).unwrap_or(&0);
        if entries != 1 { return Ok(()); } // Only do second entry after exactly one

        // Find the first entry price
        let first_price = match self.positions.get(mint_str) {
            Some(p) => p.entries.first().map(|e| e.entry_price).unwrap_or(0.0),
            None    => return Ok(()),
        };

        if first_price == 0.0 { return Ok(()); }

        let dip = (first_price - current_price) / first_price;
        if dip < self.config.second_entry_dip_pct { return Ok(()); }

        tracing::info!(
            "📌 SECOND ENTRY TRIGGER on {} | dip={:.1}%",
            &mint_str[..8.min(mint_str.len())],
            dip * 100.0
        );

        let keypair    = self.wallets.main();
        let mint       = Pubkey::from_str(mint_str)?;
        let bal        = self.engine.sol_balance(&keypair.pubkey()).await?;
        let trade_sol  = bal / 2;
        let tokens_est = (trade_sol as f64 / current_price / 1e3) as u64 * 1000;
        let max_cost   = crate::logic::max_sol_cost_with_slippage(
            trade_sol, self.config.slippage_bps,
        );

        if let Ok(sig) = self.engine.pump_buy(keypair, &mint, tokens_est, max_cost).await {
            tracing::info!("SECOND ENTRY executed: {}", sig);
            if let Some(p) = self.positions.get_mut(mint_str) {
                p.entries.push(PositionEntry {
                    sol_spent:     trade_sol,
                    tokens_bought: tokens_est,
                    entry_price:   current_price,
                });
            }
            *self.entry_counts.entry(mint_str.to_string()).or_insert(0) += 1;
        }
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Exit position
    // ─────────────────────────────────────────────────────────────────────────

    async fn exit_position(&mut self, mint_str: &str, price_sol: f64, full_exit: bool) -> Result<()> {
        let keypair = self.wallets.main();
        let mint    = Pubkey::from_str(mint_str)?;

        let balance = self.engine.token_balance(&keypair.pubkey(), &mint).await?;
        if balance == 0 {
            tracing::warn!("No tokens to sell for {}", &mint_str[..8.min(mint_str.len())]);
            if let Some(p) = self.positions.get_mut(mint_str) { p.is_closed = true; }
            return Ok(());
        }

        // Moonbag: keep a slice unless it's a stop-loss (full exit)
        let sell_amount = if full_exit || self.config.moonbag_pct == 0.0 {
            balance
        } else {
            let keep = (balance as f64 * self.config.moonbag_pct) as u64;
            balance.saturating_sub(keep)
        };

        let min_sol = crate::logic::min_amount_out_after_slippage(
            (sell_amount as f64 * price_sol * 1e-6 * 1e9) as u64,
            self.config.slippage_bps,
        );

        match self.engine.pump_sell(keypair, &mint, sell_amount, min_sol).await {
            Ok(sig) => {
                tracing::info!("SELL executed: {} | amount={}", sig, sell_amount);
                if let Some(p) = self.positions.get_mut(mint_str) {
                    p.is_closed = full_exit || sell_amount == balance;
                }
            }
            Err(e) => tracing::error!("SELL failed: {}", e),
        }
        Ok(())
    }
}
