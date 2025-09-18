use std::{collections::HashMap, sync::Arc, time::Duration};

use alloy_primitives::{hex, Address, U256};
use alloy_provider::RootProvider;
use serde_json::json;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::erc20_contract::Erc20Contract;
use super::servo_bid_coordinator::ServoBidCoordinator;
use super::servo_http_client::{PaymentChannelState, ServoHttpClient};
use super::servo_stake_contract::ServoStakeContract;

// Type of slot/block context events published by live_builder
pub type BlockCtxEvent = (
    crate::building::BlockBuildingContext,
    std::time::Duration,
    crate::live_builder::payload_events::MevBoostSlotData,
);

#[derive(Debug, Clone, Copy)]
pub struct BidderConfig {
    pub max_payment_ratio: f64, // fraction of competition to pay via SERVO (e.g., 0.5)
    pub min_payment_wei: U256,  // threshold to avoid dust
    pub max_payment_cap_wei: Option<U256>, // hard cap
    pub tick_ms: u64,           // evaluation interval
}

impl Default for BidderConfig {
    fn default() -> Self {
        Self {
            max_payment_ratio: 0.5,
            min_payment_wei: U256::from(1_000_000_000u64),
            max_payment_cap_wei: None,
            tick_ms: 3_000,
        }
    }
}

pub struct ServoBidder {
    client: ServoHttpClient,
    stake: ServoStakeContract<RootProvider>,
    #[allow(dead_code)]
    token: Erc20Contract<RootProvider>,
    staking_secret: secp256k1::SecretKey,
    staker_address: Address,
    coordinator: Arc<ServoBidCoordinator>,
    ctx_rx: broadcast::Receiver<BlockCtxEvent>,
    last_payment_by_slot: Arc<Mutex<HashMap<(u64, u64), U256>>>,
    cfg: BidderConfig,
}

impl ServoBidder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: ServoHttpClient,
        stake: ServoStakeContract<RootProvider>,
        token: Erc20Contract<RootProvider>,
        staking_secret: secp256k1::SecretKey,
        staker_address: Address,
        coordinator: Arc<ServoBidCoordinator>,
        ctx_rx: broadcast::Receiver<BlockCtxEvent>,
        cfg: Option<BidderConfig>,
    ) -> Self {
        Self {
            client,
            stake,
            token,
            staking_secret,
            staker_address,
            coordinator,
            ctx_rx,
            last_payment_by_slot: Arc::new(Mutex::new(HashMap::new())),
            cfg: cfg.unwrap_or_default(),
        }
    }

    pub fn start(mut self, cancel: CancellationToken) {
        tokio::spawn(async move {
            if let Err(e) = self.run(cancel.clone()).await {
                warn!(target="plugins", name="servo-http-bidder", error=?e, "SERVO bidder stopped with error");
            }
        });
    }

    async fn run(&mut self, cancel: CancellationToken) -> eyre::Result<()> {
        // Ensure payment channel open and staking ok
        let mut pcs = match self.client.get_payment_channel_state().await {
            Ok(p) => p,
            Err(_) => {
                info!(
                    target = "plugins",
                    name = "servo-http-bidder",
                    "opening SERVO payment channel"
                );
                self.client.open_payment_channel().await?
            }
        };
        if pcs.staked_amount.is_zero() {
            eyre::bail!("No SERVO stake for staker {:#x}", self.staker_address);
        }
        // Touch token early to avoid dead_code warnings
        let _ = self
            .token
            .allowance(self.staker_address, self.staker_address)
            .await
            .unwrap_or_default();
        info!(target="plugins", name="servo-http-bidder", staked=?pcs.staked_amount, "SERVO stake verified");

        let mut latest_ctx: Option<BlockCtxEvent> = None;
        let tick = Duration::from_millis(self.cfg.tick_ms);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                recv_res = self.ctx_rx.recv() => {
                    match recv_res {
                        Ok(ctx) => {
                            // Reset per-slot state using payload block+slot
                            let block = ctx.2.block();
                            let slot = ctx.2.slot();
                            self.last_payment_by_slot.lock().await.remove(&(block, slot));
                            latest_ctx = Some(ctx);
                            debug!(target="plugins", name="servo-http-bidder", slot=slot, block=block, "new slot context received");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            // just continue to wait for the next message
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(tick) => {
                    if let Some(ctx) = latest_ctx.clone() {
                        if let Err(e) = self.evaluate_and_bid(&mut pcs, &ctx).await {
                            warn!(target="plugins", name="servo-http-bidder", error=?e, "evaluation failed");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn evaluate_and_bid(
        &self,
        pcs: &mut PaymentChannelState,
        ctx: &BlockCtxEvent,
    ) -> eyre::Result<()> {
        let block = ctx.2.block();
        let slot = ctx.2.slot();
        // Read competition for this (block, slot)
        let competition = self.coordinator.latest_bid(block, slot);
        if competition.is_none() {
            return Ok(());
        }
        let competition = competition.unwrap();

        // Compute max payment based on policy
        let ratio_n = (self.cfg.max_payment_ratio * 1e9f64) as u64; // 1e9 fixed point
        let mut max_payment = (competition * U256::from(ratio_n)) / U256::from(1_000_000_000u64);
        if let Some(cap) = self.cfg.max_payment_cap_wei {
            max_payment = max_payment.min(cap);
        }
        if max_payment < self.cfg.min_payment_wei {
            return Ok(());
        }

        // Avoid redundant submits for this slot
        {
            let guard = self.last_payment_by_slot.lock().await;
            if guard.get(&(block, slot)).copied().unwrap_or_default() >= max_payment {
                return Ok(());
            }
        }

        // Build new commitment hash using contract helper
        let latest = &pcs.latest_commitment;
        let new_hash = self
            .stake
            .get_stake_commitment_hash(
                latest.staker_address,
                latest.stake_channel_nonce,
                latest.stake_commitment_nonce + U256::from(1),
                latest.latest_commitment_hash,
                latest.stake_spent_amount + max_payment,
            )
            .await?;

        // Sign preimage hash directly (placeholder; depends on contract requirements)
        let secp = secp256k1::Secp256k1::new();
        let msg = secp256k1::Message::from_digest(*new_hash);
        let sk = &self.staking_secret;
        let sig = secp.sign_ecdsa(&msg, sk);
        let rsig = sig.serialize_compact();
        let mut sig_bytes = [0u8; 65];
        sig_bytes[..64].copy_from_slice(&rsig[..]);
        sig_bytes[64] = 27; // placeholder 'v'
        let staker_signature = format!("0x{}", hex::encode(sig_bytes));

        // Prepare commitment payload
        let new_commitment = json!({
            "instantUnstakeData": null,
            "latestCommitmentHash": format!("0x{:x}", new_hash),
            "previousCommitmentHash": format!("0x{:x}", latest.latest_commitment_hash),
            "stakeChannelNonce": format!("0x{:x}", latest.stake_channel_nonce),
            "stakeCommitmentNonce": format!("0x{:x}", latest.stake_commitment_nonce + U256::from(1)),
            "stakeNotarySignature": null,
            "stakeSpentAmount": format!("0x{:x}", latest.stake_spent_amount + max_payment),
            "stakerAddress": format!("{:#x}", latest.staker_address),
            "stakerSignature": staker_signature,
        });

        // Submit bid
        let bid_payload = json!({
            "fees": { "valuation": format!("0x{:x}", competition) },
            "maxPayment": format!("0x{:x}", max_payment),
            "commitment": new_commitment,
        });
        debug!(target="plugins", name="servo-http-bidder", slot=slot, block=block, max_payment=?max_payment, "Submitting SERVO bid");
        let _ = self.client.bid(&bid_payload).await?;

        // Update local PCS snapshot
        pcs.latest_commitment.latest_commitment_hash = new_hash;
        pcs.latest_commitment.stake_commitment_nonce += U256::from(1);
        pcs.latest_commitment.stake_spent_amount += max_payment;
        {
            let mut guard = self.last_payment_by_slot.lock().await;
            guard.insert((block, slot), max_payment);
        }
        Ok(())
    }
}
