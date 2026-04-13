// src/logic.rs
// Pure maths: AMM quotes, slippage, price impact, PnL tracking.
// No I/O, fully unit-testable.

// ─────────────────────────────────────────────────────────────────────────────
//  Pool / Quote types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PoolReserves {
    pub reserve_in:  u128, // Token you are sending
    pub reserve_out: u128, // Token you are receiving
}

#[derive(Debug, Clone)]
pub struct SwapQuote {
    pub expected_amount_out: u64,
    pub min_amount_out:      u64,  // After slippage
    pub price_impact_bps:    u16,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Pump.fun bonding-curve state (read from on-chain account)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PumpCurveState {
    pub virtual_sol_reserves:   u64,
    pub virtual_token_reserves: u64,
    pub real_sol_reserves:      u64,
    pub real_token_reserves:    u64,
    pub token_total_supply:     u64,
    pub complete:               bool,
}

impl Default for PumpCurveState {
    fn default() -> Self {
        // Pump.fun initialises every curve with these constants
        Self {
            virtual_sol_reserves:   30_000_000_000,       // 30 SOL
            virtual_token_reserves: 1_073_000_000_000_000, // 1.073B tokens (6 dec)
            real_sol_reserves:      0,
            real_token_reserves:    793_100_000_000_000,   // ~793M tokens available
            token_total_supply:     1_000_000_000_000_000,
            complete:               false,
        }
    }
}

impl PumpCurveState {
    /// SOL cost to purchase `token_amount` tokens (in lamports).
    pub fn buy_price(&self, token_amount: u64) -> Option<u64> {
        if self.complete { return None; }
        let ta = token_amount as u128;
        let vsr = self.virtual_sol_reserves as u128;
        let vtr = self.virtual_token_reserves as u128;
        if ta >= vtr { return None; }
        // P = (vsr * ta) / (vtr - ta)   then round up
        let numerator   = vsr.checked_mul(ta)?;
        let denominator = vtr.checked_sub(ta)?;
        let price       = numerator.checked_div(denominator)?;
        Some((price + 1) as u64) // +1 = ceiling division (conservative)
    }

    /// Tokens received when spending `sol_lamports`.
    pub fn tokens_for_sol(&self, sol_lamports: u64) -> Option<u64> {
        if self.complete { return None; }
        let sol = sol_lamports as u128;
        let vsr = self.virtual_sol_reserves as u128;
        let vtr = self.virtual_token_reserves as u128;
        // t = (vtr * sol) / (vsr + sol)
        let numerator   = vtr.checked_mul(sol)?;
        let denominator = vsr.checked_add(sol)?;
        Some((numerator / denominator) as u64)
    }

    /// Current token price in SOL per 1_000_000 tokens (6-dec normalised).
    pub fn price_per_token(&self) -> f64 {
        if self.virtual_token_reserves == 0 { return 0.0; }
        let vsr = self.virtual_sol_reserves as f64 / 1e9;   // in SOL
        let vtr = self.virtual_token_reserves as f64 / 1e6; // in tokens (6 dec)
        vsr / vtr
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Generic AMM (Raydium / Orca CLMM approximation)
// ─────────────────────────────────────────────────────────────────────────────

pub struct Logic;

impl Logic {
    /// Constant-product AMM quote.  Fee is Raydium V4 default: 0.25 % (25 bps).
    pub fn calculate_amm_quote(
        amount_in:    u64,
        reserves:     &PoolReserves,
        slippage_bps: u16,
    ) -> Option<SwapQuote> {
        if reserves.reserve_in == 0 || reserves.reserve_out == 0 {
            return None;
        }
        let amount_in_u128 = amount_in as u128;
        // Deduct 0.25 % fee
        let amount_after_fee = (amount_in_u128 * 9975) / 10_000;
        // dy = (y * dx) / (x + dx)
        let numerator          = amount_after_fee * reserves.reserve_out;
        let denominator        = reserves.reserve_in + amount_after_fee;
        let expected_out       = (numerator / denominator) as u64;
        let min_out            = min_amount_out_after_slippage(expected_out, slippage_bps);
        let price_impact_bps   = ((amount_in_u128 * 10_000) / reserves.reserve_in) as u16;

        Some(SwapQuote {
            expected_amount_out: expected_out,
            min_amount_out:      min_out,
            price_impact_bps,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Slippage helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Minimum tokens / SOL to accept when selling.
#[must_use]
pub fn min_amount_out_after_slippage(quoted_out: u64, slippage_bps: u16) -> u64 {
    let factor = 10_000u128.saturating_sub(slippage_bps as u128);
    ((quoted_out as u128 * factor) / 10_000) as u64
}

/// Maximum SOL to spend when buying (with slippage headroom).
#[must_use]
pub fn max_sol_cost_with_slippage(quoted_sol: u64, slippage_bps: u16) -> u64 {
    let factor = 10_000u128 + slippage_bps as u128;
    ((quoted_sol as u128 * factor) / 10_000) as u64
}

// ─────────────────────────────────────────────────────────────────────────────
//  Position / PnL tracker
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Position {
    pub mint:           String,
    pub entries:        Vec<PositionEntry>,  // up to max_entries_per_token
    pub peak_price_sol: f64,                 // for trailing stop
    pub is_closed:      bool,
}

#[derive(Debug, Clone)]
pub struct PositionEntry {
    pub sol_spent:     u64,
    pub tokens_bought: u64,
    pub entry_price:   f64,
}

impl Position {
    pub fn new(mint: &str) -> Self {
        Self {
            mint:           mint.to_string(),
            entries:        Vec::new(),
            peak_price_sol: 0.0,
            is_closed:      false,
        }
    }

    /// Average cost basis in SOL per token (6-dec normalised).
    pub fn avg_cost(&self) -> f64 {
        let total_sol: u64    = self.entries.iter().map(|e| e.sol_spent).sum();
        let total_tokens: u64 = self.entries.iter().map(|e| e.tokens_bought).sum();
        if total_tokens == 0 { return 0.0; }
        (total_sol as f64 / 1e9) / (total_tokens as f64 / 1e6)
    }

    /// Unrealised PnL multiplier relative to average cost.
    /// Returns 1.25 when up 25 %, 0.88 when down 12 %.
    pub fn pnl_multiplier(&self, current_price_sol: f64) -> f64 {
        let cost = self.avg_cost();
        if cost == 0.0 { return 1.0; }
        current_price_sol / cost
    }

    /// Update peak price (call on every price tick).
    pub fn update_peak(&mut self, current_price_sol: f64) {
        if current_price_sol > self.peak_price_sol {
            self.peak_price_sol = current_price_sol;
        }
    }

    /// True if the trailing stop has been breached.
    pub fn trailing_stop_triggered(&self, current_price_sol: f64, trailing_pct: f64) -> bool {
        if self.peak_price_sol == 0.0 { return false; }
        current_price_sol < self.peak_price_sol * (1.0 - trailing_pct)
    }

    /// Total SOL invested across all entries.
    pub fn total_sol_spent(&self) -> u64 {
        self.entries.iter().map(|e| e.sol_spent).sum()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Daily risk state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct DailyRiskState {
    pub starting_balance_lamports: u64,
    pub realised_pnl_lamports:     i64,
    pub trade_count:               u32,
}

impl DailyRiskState {
    pub fn loss_fraction(&self) -> f64 {
        if self.starting_balance_lamports == 0 { return 0.0; }
        let loss = self.realised_pnl_lamports.min(0).unsigned_abs();
        loss as f64 / self.starting_balance_lamports as f64
    }

    pub fn limit_breached(&self, limit_pct: f64) -> bool {
        self.loss_fraction() >= limit_pct
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amm_quote() {
        let reserves = PoolReserves { reserve_in: 1_000_000_000, reserve_out: 150_000_000_000 };
        let quote = Logic::calculate_amm_quote(100_000_000, &reserves, 100).unwrap();
        assert!(quote.expected_amount_out > 0, "Expected output should be > 0");
        assert!(quote.min_amount_out < quote.expected_amount_out, "Min should be < expected");
    }

    #[test]
    fn test_pump_buy_price() {
        let curve = PumpCurveState::default();
        let price = curve.buy_price(1_000_000_000).unwrap(); // 1k tokens (6 dec)
        assert!(price > 0, "Price should be > 0");
    }

    #[test]
    fn test_slippage() {
        let out = min_amount_out_after_slippage(1_000_000, 100); // 1% slippage
        assert_eq!(out, 990_000);
    }

    #[test]
    fn test_position_pnl() {
        let mut pos = Position::new("TEST");
        pos.entries.push(PositionEntry {
            sol_spent:     100_000_000, // 0.1 SOL
            tokens_bought: 1_000_000,  // 1 token (6 dec)
            entry_price:   0.1,
        });
        let mult = pos.pnl_multiplier(0.125); // price up 25%
        assert!((mult - 1.25).abs() < 0.01);
    }
}
