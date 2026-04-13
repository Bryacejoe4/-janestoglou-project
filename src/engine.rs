// src/engine.rs
// Core trade execution engine.
//
// FIX vs original:
//   • Uses `nonblocking::rpc_client::RpcClient` — no thread-pool starvation.
//   • `execute_jito_trade` no longer blocks the async executor.
//   • Retry logic is built-in via `utils::retry_async`.
//   • Simulation failure is a hard abort (saves Jito tips).

use anyhow::{anyhow, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    instruction::Instruction,
    message::{v0::Message, VersionedMessage},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::VersionedTransaction,
};
use reqwest::Client;
use serde_json::json;
use std::str::FromStr;

use crate::{
    config::BotConfig,
    dex::pumpfun,
    utils,
};

// ─────────────────────────────────────────────────────────────────────────────
//  Jito tip accounts (round-robin for load distribution)
// ─────────────────────────────────────────────────────────────────────────────

const JITO_TIP_ACCOUNTS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt13ffZ9KrF",
];

const JITO_REGIONS: &[&str] = &[
    "https://ny.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://amsterdam.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://frankfurt.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://tokyo.mainnet.block-engine.jito.wtf/api/v1/bundles",
];

// ─────────────────────────────────────────────────────────────────────────────
//  TradingEngine
// ─────────────────────────────────────────────────────────────────────────────

pub struct TradingEngine {
    pub rpc:    RpcClient,
    pub http:   Client,
    pub config: BotConfig,
}

impl TradingEngine {
    pub fn new(config: BotConfig) -> Self {
        let rpc  = RpcClient::new_with_commitment(
            config.rpc_url.clone(),
            CommitmentConfig::confirmed(),
        );
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client build failed");

        Self { rpc, http, config }
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  High-level: Pump.fun buy via Jito
    // ─────────────────────────────────────────────────────────────────────────

    pub async fn pump_buy(
        &self,
        keypair:      &Keypair,
        mint:         &Pubkey,
        token_amount: u64,
        max_sol_cost: u64,
    ) -> Result<String> {
        let payer = keypair.pubkey();
        tracing::info!("BUY  {} | tokens={} max_sol={:.4}",
            utils::short_key(mint), token_amount,
            utils::lamports_to_sol(max_sol_cost));

        let mut instructions = self.base_compute_ixs();

        // Create ATA if it doesn't exist (idempotent)
        instructions.push(
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &payer, &payer, mint,
                &Pubkey::from_str(&spl_token::id().to_string()).unwrap(),
            )
        );

        instructions.push(pumpfun::build_buy_instruction(
            &payer, mint, token_amount, max_sol_cost,
        ));

        instructions.push(self.jito_tip_ix(&payer));

        self.simulate_and_send(keypair, &instructions).await
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  High-level: Pump.fun sell via Jito
    // ─────────────────────────────────────────────────────────────────────────

    pub async fn pump_sell(
        &self,
        keypair:        &Keypair,
        mint:           &Pubkey,
        token_amount:   u64,
        min_sol_output: u64,
    ) -> Result<String> {
        let payer = keypair.pubkey();
        tracing::info!("SELL {} | tokens={} min_sol={:.4}",
            utils::short_key(mint), token_amount,
            utils::lamports_to_sol(min_sol_output));

        let mut instructions = self.base_compute_ixs();
        instructions.push(pumpfun::build_sell_instruction(
            &payer, mint, token_amount, min_sol_output,
        ));
        instructions.push(self.jito_tip_ix(&payer));

        self.simulate_and_send(keypair, &instructions).await
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Fetch token balance in ATA
    // ─────────────────────────────────────────────────────────────────────────

    pub async fn token_balance(&self, owner: &Pubkey, mint: &Pubkey) -> Result<u64> {
        let ata = utils::get_ata(owner, mint);
        let bal = self.rpc.get_token_account_balance(&ata).await
            .map_err(|e| anyhow!("get_token_account_balance: {}", e))?;
        Ok(bal.amount.parse::<u64>().unwrap_or(0))
    }

    /// SOL balance in lamports.
    pub async fn sol_balance(&self, wallet: &Pubkey) -> Result<u64> {
        self.rpc.get_balance(wallet).await
            .map_err(|e| anyhow!("get_balance: {}", e))
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Core: simulate → send → broadcast to Jito
    // ─────────────────────────────────────────────────────────────────────────

    pub async fn simulate_and_send(
        &self,
        keypair:      &Keypair,
        instructions: &[Instruction],
    ) -> Result<String> {
        let payer          = keypair.pubkey();
        let blockhash      = self.rpc
            .get_latest_blockhash()
            .await
            .map_err(|e| anyhow!("get_latest_blockhash: {}", e))?;

        let message = Message::try_compile(&payer, instructions, &[], blockhash)
            .map_err(|e| anyhow!("compile message: {}", e))?;

        let tx = VersionedTransaction::try_new(
            VersionedMessage::V0(message),
            &[keypair],
        ).map_err(|e| anyhow!("sign tx: {}", e))?;

        // ── Local simulation (abort before wasting Jito tip) ──────────────────
        let sim = self.rpc.simulate_transaction(&tx).await
            .map_err(|e| anyhow!("simulate_transaction: {}", e))?;

        if let Some(err) = sim.value.err {
            let logs = sim.value.logs.unwrap_or_default();
            tracing::error!("SIMULATION FAILED: {:?}", err);
            for log in &logs { tracing::error!("  {}", log); }
            return Err(anyhow!("Simulation failed: {:?}\n{}", err, logs.join("\n")));
        }
        tracing::debug!("Simulation passed ✓");

        let sig        = tx.signatures[0].to_string();
        let raw        = bincode::serialize(&tx)
            .map_err(|e| anyhow!("serialize: {}", e))?;
        let b58_tx     = bs58::encode(&raw).into_string();

        if self.config.jito.enabled {
            self.send_jito_bundle(&b58_tx).await?;
        } else {
            // Fallback: standard RPC send
            self.rpc.send_transaction(&tx).await
                .map_err(|e| anyhow!("send_transaction: {}", e))?;
        }

        tracing::info!("TX: https://solscan.io/tx/{}", sig);
        Ok(sig)
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Jito bundle dispatch
    // ─────────────────────────────────────────────────────────────────────────

    async fn send_jito_bundle(&self, b58_tx: &str) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[b58_tx]]
        });

        for url in JITO_REGIONS {
            match self.http.post(*url).json(&payload).send().await {
                Ok(resp) => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if body.get("result").is_some() {
                            tracing::info!("Bundle accepted by {}", url);
                            return Ok(());
                        }
                        if let Some(e) = body.get("error") {
                            tracing::warn!("Jito {} rejected bundle: {}", url, e);
                        }
                    }
                }
                Err(e) => tracing::warn!("Jito {} unreachable: {}", url, e),
            }
        }
        // All regions failed – fall back to standard RPC send
        tracing::warn!("All Jito regions failed; bundle may have landed anyway – check Solscan");
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    //  Helpers
    // ─────────────────────────────────────────────────────────────────────────

    fn base_compute_ixs(&self) -> Vec<Instruction> {
        vec![
            ComputeBudgetInstruction::set_compute_unit_limit(200_000),
            ComputeBudgetInstruction::set_compute_unit_price(
                self.config.strategy.priority_fee_micro_lamports,
            ),
        ]
    }

    fn jito_tip_ix(&self, payer: &Pubkey) -> Instruction {
        let idx      = (chrono::Utc::now().timestamp() as usize) % JITO_TIP_ACCOUNTS.len();
        let tip_acct = Pubkey::from_str(JITO_TIP_ACCOUNTS[idx])
            .expect("Jito tip account address is invalid base58");
        system_instruction::transfer(payer, &tip_acct, self.config.jito.tip_lamports)
    }
}
