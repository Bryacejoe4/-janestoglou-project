# INSTRUCTIONS.md — solana-hft-botx Setup & Operation Guide

---

## 1. Prerequisites

| Tool | Install |
|------|---------|
| Rust (stable) | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Helius RPC (recommended) | https://helius.dev — free tier works; paid tier gives lower latency |
| Solana wallet | Phantom / Solflare export, or a CLI-generated key |

> **Why Helius?**  
> Standard public RPCs rate-limit heavily. Helius gives you both HTTP RPC and WebSocket (WSS) URLs, which the sniper requires. The free plan at ~50 req/s is enough for testing.

---

## 2. Project Setup

```bash
# Step 1 — enter the project folder
cd solana-hft-botx

# Step 2 — copy environment template
cp .env.example .env

# Step 3 — edit .env with your actual values
nano .env          # or use VS Code, vim, etc.
```

### Minimum `.env` (required fields)

```env
RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY
WSS_URL=wss://mainnet.helius-rpc.com/?api-key=YOUR_KEY
PRIVATE_KEY=YOUR_WALLET_PRIVATE_KEY_IN_BASE58
```

### Multi-wallet `.env`

```env
RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY
WSS_URL=wss://mainnet.helius-rpc.com/?api-key=YOUR_KEY
PRIVATE_KEYS=KEY_ONE,KEY_TWO,KEY_THREE
```

### How to export your private key

**From Phantom:**
1. Settings → Security & Privacy → Export Private Key
2. The key is a Base58 string — paste it directly as `PRIVATE_KEY`

**From Solana CLI:**
```bash
solana-keygen new --outfile ~/.config/solana/id.json
cat ~/.config/solana/id.json   # JSON array of bytes
# Convert to base58:
solana-keygen pubkey --ask-seed-phrase   # or use bs58 crate
```

---

## 3. Build

```bash
# Release build (use this for real trading — ~10x faster than debug)
cargo build --release

# Debug build (slower execution, better error messages for development)
cargo build
```

Build artifacts are in `target/release/botx` (or `target/debug/botx`).

---

## 4. Verify Everything Works

Before risking any real SOL, run the offline test suite:

```bash
./target/release/botx test
```

Expected output:
```
══════════════════════════════════════════════
  solana-hft-botx  ·  Offline Test Suite
══════════════════════════════════════════════

✅ [1/6] AMM Quote
✅ [2/6] Pump.fun Bonding Curve
✅ [3/6] Slippage Helpers
✅ [4/6] Token Filter – should PASS
✅ [5/6] Token Filter – should REJECT (high snipers)
✅ [6/6] Risk Manager

══════════════════════════════════════════════
  ✅ ALL 6 TESTS PASSED – Foundation is solid
══════════════════════════════════════════════
```

If any test fails, check your Cargo.toml dependencies match those in this project.

---

## 5. Check Wallet Balance

```bash
./target/release/botx balance
```

Expected output:
```
Wallet 1 | <PUBKEY> | 1.234567 SOL
```

If you see an error here, your `RPC_URL` or `PRIVATE_KEY` is wrong.

---

## 6. Strategy Configuration

Open `config/default.toml` and adjust to your risk tolerance.

### Conservative settings (recommended for first run)

```toml
[strategy]
entry_split           = true
second_entry_dip_pct  = 0.25
take_profit_pct       = 0.20   # Exit at +20%
stop_loss_pct         = 0.10   # Exit at -10%
moonbag_pct           = 0.00   # No moonbag (sell everything at TP)
trailing_stop_pct     = 0.08
slippage_bps          = 300    # 3% — wider tolerance for new tokens

[risk]
max_position_pct      = 0.10   # Only risk 10% of balance per trade
daily_loss_limit_pct  = 0.05   # Pause after 5% daily loss
max_sol_per_trade     = 100_000_000   # Hard cap at 0.1 SOL per trade

[sniper]
enabled               = true
min_volume_usd        = 50000.0   # Higher bar for entry
buy_amount_lamports   = 50_000_000  # 0.05 SOL per snipe

[jito]
tip_lamports          = 10_000_000   # 0.01 SOL tip
```

### Aggressive settings (experienced users only)

```toml
[strategy]
take_profit_pct  = 0.30
stop_loss_pct    = 0.15
moonbag_pct      = 0.15
slippage_bps     = 500

[risk]
max_position_pct = 0.15
max_sol_per_trade = 500_000_000   # 0.5 SOL
```

---

## 7. Running the Bot

### Full automated mode

```bash
./target/release/botx run
```

This starts three concurrent tasks:
1. **Sniper** — listens for new Pump.fun token launches
2. **Copy trader** — mirrors configured wallets (if `copy_trade.enabled = true`)
3. **Strategy** — receives signals, applies filters, executes trades

### Read-only scan (no trades)

```bash
./target/release/botx scan
```

Watch Pump.fun activity without placing any orders. Good for monitoring.

### Manual trading

```bash
# Buy 0.1 SOL worth of a token
./target/release/botx buy <MINT_ADDRESS> --lamports 100000000

# Buy a custom amount (0.05 SOL)
./target/release/botx buy <MINT_ADDRESS> --lamports 50000000

# Sell all tokens (no slippage protection — instant dump)
./target/release/botx sell <MINT_ADDRESS>

# Sell with slippage protection (minimum 0.05 SOL output)
./target/release/botx sell <MINT_ADDRESS> --min-sol 50000000
```

---

## 8. Copy Trading Setup

To mirror another trader's buys:

1. In `config/default.toml`:
```toml
[copy_trade]
enabled          = true
watched_wallets  = ["WALLET_ADDRESS_1", "WALLET_ADDRESS_2"]
trade_size_pct   = 0.50   # Mirror at 50% of their trade size
```

2. The bot will subscribe to those wallets via WebSocket and fire entry signals whenever they buy on Pump.fun.

---

## 9. Logging & Monitoring

```bash
# Default output (info level — trade signals and results)
./target/release/botx run

# Verbose (see all filter decisions, price ticks, simulation details)
RUST_LOG=botx=debug ./target/release/botx run

# Quiet (only warnings and errors)
RUST_LOG=warn ./target/release/botx run

# Save logs to file
./target/release/botx run 2>&1 | tee bot.log
```

### Key log messages to watch

| Message | Meaning |
|---------|---------|
| `✅ ENTRY SIGNAL for <MINT>` | Token passed all filters, buying |
| `🎯 TAKE PROFIT on <MINT>` | Position exited at profit target |
| `📉 TRAILING STOP on <MINT>` | Trailing stop triggered |
| `🛑 STOP LOSS on <MINT>` | Hard stop-loss triggered |
| `⚠️ Daily loss limit hit!` | Bot auto-paused |
| `❌ SIMULATION FAILED` | Transaction rejected before sending — inspect logs |
| `✅ Bundle accepted by <URL>` | Jito bundle submitted |

---

## 10. Emergency Procedures

### Stop the bot immediately
```
Ctrl+C
```
The bot will complete any in-flight transactions and exit cleanly within 5 seconds.

### Sell everything manually
```bash
# List positions first (check your wallet on Solscan)
./target/release/botx balance

# Sell a specific token
./target/release/botx sell <MINT_ADDRESS>
```

### Bot is paused after daily loss limit
The bot logs a warning and stops opening new trades. To reset:
1. Stop the bot (`Ctrl+C`)
2. Optionally increase `daily_loss_limit_pct` in `config/default.toml`
3. Restart: `./target/release/botx run`

---

## 11. Common Errors

| Error | Cause | Fix |
|-------|-------|-----|
| `RPC_URL missing in .env` | `.env` file not found or key missing | Copy `.env.example` to `.env` and fill in |
| `No private key found` | `PRIVATE_KEY` not set | Add to `.env` |
| `Wrong key length: N bytes` | Key is not valid base58 | Re-export from Phantom |
| `SIMULATION FAILED: InstructionError` | Program error (rug pulled, pool drained, wrong accounts) | Check Solscan for the token's status |
| `Jito: all regions failed` | Network congestion | Transaction may still land; check Solscan. Increase `tip_lamports`. |
| `Account data too short for bonding curve` | Token migrated to Raydium (bonding curve complete) | Token has graduated; Pump.fun sell is no longer valid. Use Raydium. |
| `token balance is 0` | ATA doesn't exist or wrong mint | Confirm mint address on Solscan |

---

## 12. Phase 2 Enhancements (Planned)

The following features have stubs in the codebase and are ready for implementation:

- **Raydium/Orca live swaps** — `dex/raydium.rs` and `dex/orca.rs` instruction builders are complete; integration into the strategy for post-graduation tokens is next
- **Holder distribution API** — `filters.rs` has `fresh_wallet_pct` / `top10_pct` fields; hook up to Helius DAS or Birdeye API
- **Volume oracle** — plug Birdeye or DexScreener price/volume API into `monitor.rs`
- **Multi-wallet rotation** — `WalletManager::all()` is available; strategy can round-robin across wallets
- **Web dashboard** — replace CLI with a simple Axum HTTP server + HTML status page

---

## 13. File Structure Reference

```
solana-hft-botx/
├── Cargo.toml              Dependencies
├── .env.example            Credentials template
├── .gitignore              Prevents .env from being committed
├── README.md               Project overview
├── INSTRUCTIONS.md         This file
├── config/
│   └── default.toml        All strategy/risk/sniper parameters
└── src/
    ├── main.rs             CLI entry point (Run/Buy/Sell/Balance/Test/Scan)
    ├── config.rs           Config loader (TOML + .env overlay)
    ├── wallet.rs           Key loading: 32-byte seed + 64-byte keypair
    ├── utils.rs            ATA, retry_async, lamports_to_sol, short_key
    ├── logic.rs            AMM math, Pump curve, PnL, slippage (pure, testable)
    ├── engine.rs           TradingEngine: simulate → sign → Jito bundle
    ├── sniper.rs           Pump.fun WebSocket log subscription
    ├── monitor.rs          Price polling, risk signal detection
    ├── copy_trade.rs       Wallet mirroring via log subscription
    ├── dex/
    │   ├── mod.rs
    │   ├── pumpfun.rs      Buy/sell ix builders + PDA derivation
    │   ├── orca.rs         Whirlpool swap ix builder
    │   └── raydium.rs      AMM V4 swap ix builder
    └── strategy/
        ├── mod.rs
        ├── filters.rs      Token quality gate (8 checks)
        ├── risk.rs         Position sizing, daily-loss circuit breaker
        └── gembot.rs       Main strategy: split entry, TP, trailing stop, SL
```
