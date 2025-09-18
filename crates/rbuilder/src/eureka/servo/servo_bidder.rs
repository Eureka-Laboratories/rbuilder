use std::{collections::HashMap, sync::Arc, time::Duration};

use alloy_primitives::{hex, keccak256, Address, B256, U256};
use alloy_provider::RootProvider;
use alloy_signer::SignerSync;
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
    staking_signer: alloy_signer_local::PrivateKeySigner,
    staker_address: Address,
    coordinator: Arc<ServoBidCoordinator>,
    ctx_rx: broadcast::Receiver<BlockCtxEvent>,
    last_payment_by_slot: Arc<Mutex<HashMap<(u64, u64), U256>>>,
    slot_spent_base: Arc<Mutex<Option<U256>>>,
    cfg: BidderConfig,
    // EIP-712 domain params
    chain_id: u64,
}

impl ServoBidder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: ServoHttpClient,
        stake: ServoStakeContract<RootProvider>,
        token: Erc20Contract<RootProvider>,
        staking_signer: alloy_signer_local::PrivateKeySigner,
        staker_address: Address,
        coordinator: Arc<ServoBidCoordinator>,
        ctx_rx: broadcast::Receiver<BlockCtxEvent>,
        chain_id: u64,
        cfg: Option<BidderConfig>,
    ) -> Self {
        Self {
            client,
            stake,
            token,
            staking_signer,
            staker_address,
            coordinator,
            ctx_rx,
            last_payment_by_slot: Arc::new(Mutex::new(HashMap::new())),
            slot_spent_base: Arc::new(Mutex::new(None)),
            cfg: cfg.unwrap_or_default(),
            chain_id,
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
                            // Try to fetch channel state to record per-slot base
                            match self.client.get_payment_channel_state().await {
                                Ok(state) => {
                                    let base = state.latest_commitment.stake_spent_amount;
                                    *self.slot_spent_base.lock().await = Some(base);
                                    debug!(target="plugins", name="servo-http-bidder", slot=slot, block=block, base=%base, "SERVO: set slot_spent_base at slot start");
                                }
                                Err(err) => {
                                    warn!(target="plugins", name="servo-http-bidder", error=?err, "SERVO: failed to fetch payment channel state at slot start; disabling committed adjustment this slot");
                                    *self.slot_spent_base.lock().await = None;
                                }
                            }
                            latest_ctx = Some(ctx);
                            debug!(target="plugins", name="servo-http-bidder", slot=slot, block=block, "new slot context received");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            // just continue to wait for the next message
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            info!(target="plugins", name="servo-http-bidder", "Block context receiver closed");
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
            debug!(
                target = "plugins",
                name = "servo-http-bidder",
                block,
                slot,
                "No competition available for this slot"
            );
            return Ok(());
        }
        let competition = competition.unwrap();
        debug!(target="plugins", name="servo-http-bidder", block, slot, competition = %competition, "Competition snapshot");

        // Compute max payment based on policy
        let ratio_n = (self.cfg.max_payment_ratio * 1e9f64) as u64; // 1e9 fixed point
        let mut max_payment = (competition * U256::from(ratio_n)) / U256::from(1_000_000_000u64);
        if let Some(cap) = self.cfg.max_payment_cap_wei {
            max_payment = max_payment.min(cap);
        }
        if max_payment < self.cfg.min_payment_wei {
            debug!(target="plugins", name="servo-http-bidder", block, slot, max_payment = %max_payment, min = %self.cfg.min_payment_wei, "Computed max payment below min threshold; skipping");
            return Ok(());
        }

        // Avoid redundant submits for this slot
        {
            let guard = self.last_payment_by_slot.lock().await;
            if guard.get(&(block, slot)).copied().unwrap_or_default() >= max_payment {
                debug!(target="plugins", name="servo-http-bidder", block, slot, max_payment = %max_payment, "Skipping duplicate-or-smaller payment for this slot");
                return Ok(());
            }
        }

        // Build new commitment hash using contract helper
        let latest = &pcs.latest_commitment;
        let new_spent = latest.stake_spent_amount + max_payment;
        let new_nonce = latest.stake_commitment_nonce + U256::from(1);
        // Safety check: ensure on-chain staked covers new_spent
        let on_chain_staked = self
            .stake
            .get_staked_amount(self.staker_address)
            .await
            .unwrap_or_default();
        if new_spent > on_chain_staked {
            warn!(target="plugins", name="servo-http-bidder", on_chain_staked=%on_chain_staked, needed=%new_spent, "Insufficient stake; skipping SERVO bid this slot");
            return Ok(());
        }
        // Log current allowance against stake contract (spender = stake contract)
        if let Ok(allow) = self
            .token
            .allowance(self.staker_address, self.stake.address)
            .await
        {
            debug!(target="plugins", name="servo-http-bidder", allowance=%allow, spender=%format!("{:#x}", self.stake.address), "Current ERC20 allowance");
        }
        let new_hash = self
            .stake
            .get_stake_commitment_hash(
                latest.staker_address,
                latest.stake_channel_nonce,
                new_nonce,
                latest.latest_commitment_hash,
                new_spent,
            )
            .await?;

        // Build EIP-712 typed data hash for StakeCommitment and sign it
        let domain_separator = self.eip712_domain_separator(self.stake.address);
        let message_hash = Self::stake_commitment_message_hash(
            latest.staker_address,
            new_spent,
            new_nonce,
            latest.stake_channel_nonce,
            latest.latest_commitment_hash,
        );
        let mut data = Vec::with_capacity(66);
        data.extend_from_slice(b"\x19\x01");
        data.extend_from_slice(domain_separator.as_slice());
        data.extend_from_slice(message_hash.as_slice());
        let digest = keccak256(data);

        // Sign via alloy_signer_local to mirror ebuilder
        let staker_signature = {
            let sig = self
                .staking_signer
                .sign_hash_sync(&digest)
                .map_err(|e| eyre::eyre!("signing error: {e}"))?;
            format!("0x{}", hex::encode(sig.as_bytes()))
        };

        // Prepare commitment payload
        let new_commitment = json!({
            "instantUnstakeData": null,
            "latestCommitmentHash": format!("0x{:x}", new_hash),
            "previousCommitmentHash": format!("0x{:x}", latest.latest_commitment_hash),
            "stakeChannelNonce": format!("0x{:x}", latest.stake_channel_nonce),
            "stakeCommitmentNonce": format!("0x{:x}", new_nonce),
            "stakeNotarySignature": null,
            "stakeSpentAmount": format!("0x{:x}", new_spent),
            "stakerAddress": format!("{:#x}", latest.staker_address),
            "stakerSignature": staker_signature,
        });

        // Submit bid
        let bid_payload = json!({
            "fees": { "valuation": format!("0x{:x}", competition) },
            "maxPayment": format!("0x{:x}", max_payment),
            "commitment": new_commitment,
        });
        debug!(target="plugins", name="servo-http-bidder", slot=slot, block=block, max_payment=%max_payment, "Submitting SERVO bid");
        let _ = self.client.bid(&bid_payload).await?;

        // Update local PCS snapshot
        pcs.latest_commitment.latest_commitment_hash = new_hash;
        pcs.latest_commitment.stake_commitment_nonce = new_nonce;
        pcs.latest_commitment.stake_spent_amount = new_spent;
        {
            let mut guard = self.last_payment_by_slot.lock().await;
            guard.insert((block, slot), max_payment);
        }
        Ok(())
    }

    fn eip712_domain_separator(&self, verifying_contract: Address) -> B256 {
        // EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)
        let domain_type_hash = keccak256(
            "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
                .as_bytes(),
        );
        let name_hash = keccak256("SERVO".as_bytes());
        let version_hash = keccak256(b"1");
        let mut encoded = Vec::with_capacity(32 * 5);
        encoded.extend_from_slice(domain_type_hash.as_slice());
        encoded.extend_from_slice(name_hash.as_slice());
        encoded.extend_from_slice(version_hash.as_slice());
        // chainId as 32 bytes
        let mut chain_id_bytes = [0u8; 32];
        let be = self.chain_id.to_be_bytes();
        chain_id_bytes[32 - be.len()..].copy_from_slice(&be);
        encoded.extend_from_slice(&chain_id_bytes);
        // verifyingContract as 32-byte left-padded
        let mut addr = [0u8; 32];
        addr[12..].copy_from_slice(verifying_contract.as_slice());
        encoded.extend_from_slice(&addr);
        keccak256(encoded)
    }

    fn stake_commitment_message_hash(
        staker_address: Address,
        stake_spent_amount: U256,
        stake_commitment_nonce: U256,
        stake_channel_nonce: U256,
        previous_commitment_hash: B256,
    ) -> B256 {
        let type_hash = keccak256(b"StakeCommitment(address stakerAddress,uint256 stakeSpentAmount,uint256 stakeCommitmentNonce,uint256 stakeChannelNonce,bytes32 previousCommitmentHash)");
        let mut encoded = Vec::with_capacity(32 * 6);
        encoded.extend_from_slice(type_hash.as_slice());
        // stakerAddress
        let mut addr_bytes = [0u8; 32];
        addr_bytes[12..].copy_from_slice(staker_address.as_slice());
        encoded.extend_from_slice(&addr_bytes);
        // stakeSpentAmount
        encoded.extend_from_slice(B256::from(stake_spent_amount).as_slice());
        // stakeCommitmentNonce
        encoded.extend_from_slice(B256::from(stake_commitment_nonce).as_slice());
        // stakeChannelNonce
        encoded.extend_from_slice(B256::from(stake_channel_nonce).as_slice());
        // previousCommitmentHash
        encoded.extend_from_slice(previous_commitment_hash.as_slice());
        keccak256(encoded)
    }
}
