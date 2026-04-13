# solana-hft-botx

A high-performance, fully automated Solana meme-coin trading bot.  
Replicates and improves the [Gembot](https://gembot.io/) signal strategy programmatically — eliminating manual reaction time.

---

## Features

| Module | Description |
|--------|-------------|
| **Sniper** | Detects new Pump.fun token launches via WebSocket log subscription. Fires entry signals in milliseconds. |
| **Gembot Strategy** | Fully automated replication: token filters, split entry (50/50), second entry on dip, take-profit, trailing stop, moonbag, hard stop-loss. |
| **Risk Manager** | Per-trade position sizing (max % of balance), daily-loss circuit-breaker (auto-pause), hard SOL cap per trade. |
| **Copy Trading** | Subscribes to selected wallets. Mirrors buy signals with configurable size ratio. |
| **Market Monitor** | Polls bonding-curve accounts for live prices. Emits risk alerts on sudden pumps/dumps. |
| **DEX Layer** | Pump.fun, Raydium AMM V4, Orca Whirlpool — raw instruction builders with no external SDK dependency. |
| **Jito Integration** | Multi-region bundle dispatch with local simulation gate (no wasted tips on broken txs). |
| **Multi-wallet** | Load and round-robin across any number of wallets from `.env`. |

---

## Quick Start

```bash
# 1. Clone / copy the project
cd solana-hft-botx

# 2. Install Rust (if needed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 3. Set up credentials
cp .env.example .env
# Edit .env — fill in RPC_URL, WSS_URL, PRIVATE_KEY

# 4. Build (release mode is ~10x faster)
cargo build --release

# 5. Verify everything works (offline, no real trades)
./target/release/botx test

# 6. Check your balance
./target/release/botx balance

# 7. Start the bot
./target/release/botx run
```

---

## CLI Commands

```
botx run                          # Full automated mode
botx buy  <MINT> [--lamports N]   # Manual buy
botx sell <MINT> [--min-sol N]    # Manual sell all
botx balance                      # Show all wallet balances
botx scan                         # Read-only Pump.fun activity feed
botx test                         # Offline sanity tests
```

---

## Configuration

All strategy parameters live in **`config/default.toml`** — no code changes needed.

Key parameters:

```toml
[strategy]
entry_split           = true    # 50/50 split entry
second_entry_dip_pct  = 0.25   # Enter second half after 25% dip
take_profit_pct       = 0.25   # Exit at +25%
stop_loss_pct         = 0.12   # Exit at -12%
moonbag_pct           = 0.10   # Keep 10% after take-profit
trailing_stop_pct     = 0.08   # Trail 8% from peak
slippage_bps          = 200    # 2% slippage

[risk]
max_position_pct      = 0.15   # Max 15% of balance per trade
daily_loss_limit_pct  = 0.10   # Auto-pause at -10% daily
max_sol_per_trade     = 500_000_000  # Hard cap 0.5 SOL

[sniper]
enabled               = true
min_volume_usd        = 30000.0
buy_amount_lamports   = 100_000_000  # 0.1 SOL per snipe
```

Individual values can also be overridden in `.env`:
```
SLIPPAGE_BPS=300
JITO_TIP_LAMPORTS=20000000
MAX_SOL_PER_TRADE=250000000
```

---

## Architecture

```
main.rs  (CLI router)
    │
    ├── sniper.rs       ← WebSocket log listener → StrategyEvent::NewToken
    ├── copy_trade.rs   ← Wallet watcher        → StrategyEvent::NewToken
    ├── monitor.rs      ← Price poller          → StrategyEvent::PriceTick
    │
    └── strategy/
        ├── gembot.rs   ← Main event loop (mpsc receiver)
        │                 handle_new_token → filters → buy
        │                 handle_price_tick → TP / trailing-stop / SL / second-entry
        ├── filters.rs  ← Token quality gate (volume, holders, snipers, age…)
        └── risk.rs     ← Position sizing, daily loss limit

    engine.rs           ← simulate_and_send (nonblocking RpcClient + Jito)
    logic.rs            ← Pure maths: AMM, pump curve, PnL, slippage
    dex/                ← Raw ix builders (pumpfun, orca, raydium)
    wallet.rs           ← Key loading (32-byte seed + 64-byte keypair)
    config.rs           ← TOML + .env layered config
```

---

## Logging

```bash
# Default (info level)
./target/release/botx run

# Verbose debug output
RUST_LOG=botx=debug ./target/release/botx run

# Quiet (warnings and errors only)
RUST_LOG=warn ./target/release/botx run
```

---

## Security

- **Never commit `.env`** — it is listed in `.gitignore`
- Private keys are loaded only into memory and never logged
- Simulation gate prevents sending known-bad transactions to Jito
- Daily loss limit auto-pauses the bot to protect capital

---

## Disclaimer

This software is provided for educational purposes. Automated trading carries significant financial risk. The authors are not responsible for any financial losses. Always test with small amounts first.
