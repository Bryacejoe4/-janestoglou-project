// src/strategy/filters.rs
// Token quality filters – replicates Gembot's entry conditions.
// All checks return a typed verdict so callers can log the exact rejection reason.

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
//  Input data (populated by monitor.rs / on-chain data)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct TokenSnapshot {
    pub mint:            String,
    pub volume_usd_5m:   f64,   // Volume in the last 5 minutes
    pub volume_usd_1h:   f64,
    pub liquidity_sol:   f64,
    pub holder_count:    u32,
    pub top10_pct:       f64,   // % held by top-10 wallets (0.0–1.0)
    pub fresh_wallet_pct: f64,  // % holders whose wallet is < 7 days old
    pub sniper_bundle_pct: f64, // Estimated % bought in opening bundles
    pub age_seconds:     u64,   // How old the token is
    pub price_sol:       f64,
    pub price_change_1h: f64,   // +0.50 = up 50 %, -0.30 = down 30 %
    pub organic_chart:   bool,  // Heuristic: no obvious pump-dump shape
}

// ─────────────────────────────────────────────────────────────────────────────
//  Filter result
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FilterVerdict {
    Pass,
    Reject(String),
}

impl FilterVerdict {
    pub fn is_pass(&self) -> bool { *self == FilterVerdict::Pass }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Reject(r) => Some(r),
            Self::Pass      => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Filter config (mirrored from BotConfig::sniper / strategy)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FilterConfig {
    pub min_volume_usd_5m:    f64,
    pub min_liquidity_sol:    f64,
    pub max_fresh_wallet_pct: f64,
    pub max_sniper_bundle_pct: f64,
    pub max_top10_pct:        f64,
    pub min_holder_count:     u32,
    pub min_age_seconds:      u64,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            min_volume_usd_5m:     30_000.0,
            min_liquidity_sol:     5.0,
            max_fresh_wallet_pct:  0.30,
            max_sniper_bundle_pct: 0.15,
            max_top10_pct:         0.40,
            min_holder_count:      50,
            min_age_seconds:       30,   // At least 30 s old
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Filter runner
// ─────────────────────────────────────────────────────────────────────────────

pub struct TokenFilter {
    pub cfg: FilterConfig,
}

impl TokenFilter {
    pub fn new(cfg: FilterConfig) -> Self { Self { cfg } }

    /// Run all filters. Returns the first `Reject` or `Pass` if all pass.
    pub fn evaluate(&self, snap: &TokenSnapshot) -> FilterVerdict {
        let checks: &[(bool, &str)] = &[
            (
                snap.volume_usd_5m >= self.cfg.min_volume_usd_5m,
                &format!("volume ${:.0} < min ${:.0}", snap.volume_usd_5m, self.cfg.min_volume_usd_5m),
            ),
            (
                snap.liquidity_sol >= self.cfg.min_liquidity_sol,
                &format!("liquidity {:.1} SOL < min {:.1}", snap.liquidity_sol, self.cfg.min_liquidity_sol),
            ),
            (
                snap.fresh_wallet_pct <= self.cfg.max_fresh_wallet_pct,
                &format!("fresh wallets {:.0}% > max {:.0}%",
                    snap.fresh_wallet_pct * 100.0, self.cfg.max_fresh_wallet_pct * 100.0),
            ),
            (
                snap.sniper_bundle_pct <= self.cfg.max_sniper_bundle_pct,
                &format!("sniper/bundle {:.0}% > max {:.0}%",
                    snap.sniper_bundle_pct * 100.0, self.cfg.max_sniper_bundle_pct * 100.0),
            ),
            (
                snap.top10_pct <= self.cfg.max_top10_pct,
                &format!("top-10 concentration {:.0}% > max {:.0}%",
                    snap.top10_pct * 100.0, self.cfg.max_top10_pct * 100.0),
            ),
            (
                snap.holder_count >= self.cfg.min_holder_count,
                &format!("holders {} < min {}", snap.holder_count, self.cfg.min_holder_count),
            ),
            (
                snap.age_seconds >= self.cfg.min_age_seconds,
                &format!("token only {}s old (min {}s)", snap.age_seconds, self.cfg.min_age_seconds),
            ),
            (
                snap.organic_chart,
                "chart pattern looks inorganic (pump-dump shape)",
            ),
        ];

        for (pass, reason) in checks.iter() {
            if !pass {
                return FilterVerdict::Reject(reason.to_string());
            }
        }

        FilterVerdict::Pass
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn good_snap() -> TokenSnapshot {
        TokenSnapshot {
            mint:             "TEST".into(),
            volume_usd_5m:    35_000.0,
            volume_usd_1h:    120_000.0,
            liquidity_sol:    10.0,
            holder_count:     120,
            top10_pct:        0.30,
            fresh_wallet_pct: 0.20,
            sniper_bundle_pct: 0.10,
            age_seconds:      120,
            price_sol:        0.0001,
            price_change_1h:  0.15,
            organic_chart:    true,
        }
    }

    #[test]
    fn passes_all() {
        let f = TokenFilter::new(FilterConfig::default());
        assert_eq!(f.evaluate(&good_snap()), FilterVerdict::Pass);
    }

    #[test]
    fn rejects_low_volume() {
        let mut snap = good_snap();
        snap.volume_usd_5m = 5_000.0;
        let f = TokenFilter::new(FilterConfig::default());
        assert!(matches!(f.evaluate(&snap), FilterVerdict::Reject(_)));
    }

    #[test]
    fn rejects_high_snipers() {
        let mut snap = good_snap();
        snap.sniper_bundle_pct = 0.40;
        let f = TokenFilter::new(FilterConfig::default());
        assert!(matches!(f.evaluate(&snap), FilterVerdict::Reject(_)));
    }
}
