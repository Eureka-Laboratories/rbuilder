use crate::plugin::{Named, OrderInputHook, RpcDeps, RpcHook};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

use crate::live_builder::order_input::ReplaceableOrderPoolCommand;
use crate::primitives::{
    Bundle, MempoolTx, Order, TransactionSignedEcRecoveredWithBlobs, LAST_BUNDLE_VERSION,
};
use crate::utils::Signer;
use alloy_consensus::{TxEip1559, TxLegacy};
use alloy_primitives::{Address, Bytes, TxHash, B256};
use alloy_rlp::Decodable;
use serde::Deserialize;
use serde::Serialize;
use std::str::FromStr;

/// Minimal SERVO WebSocket fetcher plugin: connects to SERVO WS and injects incoming
/// txs/bundles/allocations as Orders into the order pool via ReplaceableOrderPoolCommand::Order.
#[derive(Debug, Clone)]
pub struct ServoWsFetcherPlugin {
    cfg: Option<crate::plugins::config::ServoPluginConfig>,
    sender: Arc<TokioMutex<Option<tokio::sync::mpsc::Sender<ReplaceableOrderPoolCommand>>>>,
}

impl ServoWsFetcherPlugin {
    pub fn from_plugins_config() -> Self {
        let cfg = crate::plugins::config::load_default_plugins_config().and_then(|c| c.servo);
        Self {
            cfg,
            sender: Arc::new(TokioMutex::new(None)),
        }
    }
}

impl Named for ServoWsFetcherPlugin {
    fn name(&self) -> &'static str {
        "servo-ws-fetcher"
    }
}

#[async_trait]
impl OrderInputHook for ServoWsFetcherPlugin {
    async fn on_order_input_started(&self, cancel: CancellationToken) -> eyre::Result<()> {
        let Some(cfg) = self.cfg.clone() else {
            tracing::info!(target: "plugins", name = "servo-ws-fetcher", "SERVO not configured ([servo] section missing), skipping");
            return Ok(());
        };
        let url = cfg.ws_url.clone();
        let bearer = Some(cfg.bearer_token.clone());
        let sender = self.sender.clone();
        tracing::info!(target: "plugins", name = "servo-ws-fetcher", ?url, "starting WS client");

        // WS ingest task
        let cancel_ws = cancel.clone();
        let max_attempts = cfg.max_reconnect_attempts.unwrap_or(12) as i32;
        let delay_ms = cfg.reconnect_delay_ms.unwrap_or(5_000);
        let read_timeout = cfg.timeout_ms.unwrap_or(30_000);
        tokio::spawn(async move {
            if let Err(err) = run_ws(
                url,
                bearer,
                sender,
                cancel_ws.clone(),
                max_attempts,
                delay_ms,
                read_timeout,
            )
            .await
            {
                tracing::error!(target: "plugins", name = "servo-ws-fetcher", error = ?err, "ws client terminated with error");
            }
        });
        // Minimal HTTP client probe (ensures client is migrated and usable)
        {
            let http_url = cfg.http_url.clone();
            let token = cfg.bearer_token.clone();
            tokio::spawn(async move {
                let base = http_url
                    .parse()
                    .unwrap_or("http://localhost".parse().unwrap());
                let staker = alloy_primitives::Address::ZERO;
                match crate::eureka::servo::servo_http_client::ServoHttpClient::new(
                    token, base, staker,
                ) {
                    Ok(client) => {
                        if let Ok(info) = client.get_info().await {
                            tracing::info!(target:"plugins", name="servo-http", chain_id=info.chain_id, "SERVO HTTP reachable");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(target:"plugins", name="servo-http", error=?e, "Failed to init SERVO HTTP client")
                    }
                }
            });
        }
        // Start HTTP bidder
        {
            let http_url = cfg.http_url.clone();
            let token = cfg.bearer_token.clone();
            let el_rpc_url = cfg.el_rpc_url.clone();
            let secret = cfg.servo_secret_key.clone();
            let cancel_bidder = cancel.clone();
            tokio::spawn(async move {
                // Subscribe to live_builder slot/block context events
                let rx = crate::live_builder::subscribe_slot_context();
                // Build SERVO HTTP client and contracts
                let base: url::Url = match http_url.parse() {
                    Ok(u) => u,
                    Err(_) => return,
                };
                // Derive staker wallet from secret
                let sk_bytes = match alloy_primitives::hex::decode(secret.trim_start_matches("0x"))
                {
                    Ok(b) => b,
                    Err(_) => return,
                };
                let staking_signer =
                    match alloy_signer_local::PrivateKeySigner::from_slice(&sk_bytes) {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                let staker_address = staking_signer.address();
                let client = match crate::eureka::servo::servo_http_client::ServoHttpClient::new(
                    token,
                    base,
                    staker_address,
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(target:"plugins", name = "servo-http-bidder", error=?e, "Failed to init SERVO HTTP client");
                        return;
                    }
                };
                let info = match client.get_info().await {
                    Ok(i) => i,
                    Err(e) => {
                        tracing::warn!(target:"plugins", name = "servo-http-bidder", error=?e, "Failed to fetch SERVO info");
                        return;
                    }
                };
                let provider = crate::utils::http_provider(match el_rpc_url.parse() {
                    Ok(u) => u,
                    Err(_) => return,
                });
                let stake = crate::eureka::servo::servo_stake_contract::ServoStakeContract::new(
                    info.stake_contract,
                    provider.clone(),
                );
                let token_addr = match stake.validate_deployment().await {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::warn!(target:"plugins", name = "servo-http-bidder", error=?e, "Stake validation failed");
                        return;
                    }
                };
                let token = crate::eureka::servo::erc20_contract::Erc20Contract::new(
                    token_addr,
                    provider.clone(),
                );
                // Coordinator handle
                let coordinator = crate::eureka::servo::servo_bid_coordinator::global()
                    .unwrap_or_else(|| {
                        std::sync::Arc::new(
                            crate::eureka::servo::servo_bid_coordinator::ServoBidCoordinator::new(
                                alloy_primitives::U256::ZERO,
                            ),
                        )
                    });
                // Start bidder
                let bidder = crate::eureka::servo::servo_bidder::ServoBidder::new(
                    client,
                    stake,
                    token,
                    staking_signer,
                    staker_address,
                    coordinator,
                    rx,
                    info.chain_id,
                    None,
                );
                bidder.start(cancel_bidder.clone());
                tracing::info!(target:"plugins", name = "servo-http-bidder", "SERVO HTTP bidder started");
            });
        }
        Ok(())
    }
}

impl RpcHook for ServoWsFetcherPlugin {
    fn register(&self, _module: &mut jsonrpsee::RpcModule<()>, deps: RpcDeps) -> eyre::Result<()> {
        // capture orderpool sender for later use
        let mut guard = self.sender.blocking_lock();
        *guard = Some(deps.results);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServoTx {
    tx_hash: TxHash,
    source: String,
    #[serde(default)]
    allow_reverts: bool,
    #[serde(default)]
    is_exclusive: bool,
    raw_unsigned_tx: Bytes,
    #[serde(default)]
    tx: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "messageType", content = "message", rename_all = "camelCase")]
enum ServoWsMessage {
    IncomingTx(ServoTx),
    IncomingBundle(Vec<ServoTx>),
    #[serde(rename_all = "camelCase")]
    Allocation {
        staker_address: Address,
        bid_id: String,
        raw_signed_txs: Vec<Bytes>,
    },
}

async fn run_ws(
    url: String,
    bearer: Option<String>,
    sender: Arc<TokioMutex<Option<tokio::sync::mpsc::Sender<ReplaceableOrderPoolCommand>>>>,
    cancel: CancellationToken,
    max_attempts: i32,
    delay_ms: u64,
    read_timeout: u64,
) -> eyre::Result<()> {
    use futures::{SinkExt, StreamExt};
    use reqwest::header::{HeaderName, HeaderValue, AUTHORIZATION};
    use tokio_websockets::{ClientBuilder, Message};
    use tonic::transport::Uri;

    let mut attempts = 0;
    'outer: loop {
        if cancel.is_cancelled() {
            break;
        }
        attempts += 1;
        // Build client with optional Authorization header
        let uri: Uri = url.parse().expect("invalid ws url");
        let mut builder = ClientBuilder::from_uri(uri);
        if let Some(token) = bearer.clone() {
            let name: HeaderName = AUTHORIZATION;
            let value = HeaderValue::from_str(&format!("Bearer {token}"))
                .unwrap_or_else(|_| HeaderValue::from_static(""));
            builder = builder.add_header(name, value);
        }
        match builder.connect().await {
            Ok((mut ws, _resp)) => {
                tracing::info!(target: "plugins", name = "servo-ws-fetcher", "connected");
                // Read loop with idle timeout: if no frames, reconnect
                loop {
                    if cancel.is_cancelled() {
                        let _ = ws.send(Message::close(None, "")).await;
                        break 'outer;
                    }
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(read_timeout),
                        ws.next(),
                    )
                    .await
                    {
                        Ok(Some(Ok(msg))) => {
                            if let Some(text) = msg.as_text() {
                                if let Err(e) = handle_text(text, &sender).await {
                                    tracing::warn!(target: "plugins", name = "servo-ws-fetcher", error=?e, "failed to handle SERVO message");
                                }
                            }
                        }
                        Ok(Some(Err(e))) => {
                            tracing::warn!(target:"plugins", name = "servo-ws-fetcher", error=?e, "websocket error");
                            break;
                        }
                        Ok(None) => {
                            tracing::warn!(target:"plugins", name = "servo-ws-fetcher", "websocket stream ended");
                            break;
                        }
                        Err(_) => {
                            tracing::warn!(target:"plugins", name = "servo-ws-fetcher", "websocket idle timeout");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(target:"plugins", name="servo-ws-fetcher", attempt=attempts, error=?e, "connect failed");
            }
        }
        if attempts >= max_attempts {
            tracing::error!(target:"plugins", name="servo-ws-fetcher", "max reconnect attempts reached");
            break;
        }
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
        }
    }
    Ok(())
}

async fn handle_text(
    text: &str,
    sender: &Arc<TokioMutex<Option<tokio::sync::mpsc::Sender<ReplaceableOrderPoolCommand>>>>,
) -> eyre::Result<()> {
    match serde_json::from_str::<ServoWsMessage>(text) {
        Ok(ServoWsMessage::IncomingTx(tx)) => {
            let fake_tx = try_decode_unsigned_tx(tx.raw_unsigned_tx.clone(), tx.sender());
            if let Ok(tx) = fake_tx {
                let order = Order::Tx(MempoolTx::new(tx));
                send_order(sender, order).await?;
            }
        }
        Ok(ServoWsMessage::IncomingBundle(txs)) => {
            let decoded: Vec<_> = txs
                .into_iter()
                .filter_map(|t| {
                    let raw = t.raw_unsigned_tx.clone();
                    let sender = t.sender();
                    try_decode_unsigned_tx(raw, sender).ok()
                })
                .collect();
            if !decoded.is_empty() {
                let mut bundle = Bundle {
                    version: LAST_BUNDLE_VERSION,
                    block: None,
                    min_timestamp: None,
                    max_timestamp: None,
                    txs: decoded,
                    reverting_tx_hashes: Vec::new(),
                    dropping_tx_hashes: Vec::new(),
                    hash: Default::default(),
                    uuid: Default::default(),
                    replacement_data: None,
                    signer: None,
                    metadata: Default::default(),
                    refund: None,
                };
                bundle.hash_slow();
                send_order(sender, Order::Bundle(bundle)).await?;
            }
        }
        Ok(ServoWsMessage::Allocation { raw_signed_txs, .. }) => {
            // Convert to internal txs with fake blobs for now
            let txs: Vec<TransactionSignedEcRecoveredWithBlobs> = raw_signed_txs
                .into_iter()
                .filter_map(|raw| {
                    TransactionSignedEcRecoveredWithBlobs::decode_enveloped_with_fake_blobs(raw)
                        .ok()
                })
                .collect();
            if !txs.is_empty() {
                let mut bundle = Bundle {
                    version: LAST_BUNDLE_VERSION,
                    block: None,
                    min_timestamp: None,
                    max_timestamp: None,
                    txs,
                    reverting_tx_hashes: Vec::new(),
                    dropping_tx_hashes: Vec::new(),
                    hash: Default::default(),
                    uuid: Default::default(),
                    replacement_data: None,
                    signer: None,
                    metadata: Default::default(),
                    refund: None,
                };
                bundle.hash_slow();
                send_order(sender, Order::Bundle(bundle)).await?;
            }
        }
        Err(e) => {
            tracing::debug!(target: "plugins", name = "servo-ws-fetcher", error=?e, %text, "unrecognized message");
        }
    }
    Ok(())
}

fn try_decode_unsigned_tx(
    payload: Bytes,
    signer: Option<Address>,
) -> eyre::Result<TransactionSignedEcRecoveredWithBlobs> {
    let secret = secp256k1::SecretKey::from_slice(B256::random().as_ref())?;
    let address = signer.unwrap_or_default();
    let dummy_signer = Signer { address, secret };

    if let Ok(tx) = TxEip1559::decode(&mut &payload[..]) {
        let reth_tx: reth::primitives::Transaction = tx.into();
        let signed = dummy_signer.sign_tx(reth_tx)?;
        Ok(TransactionSignedEcRecoveredWithBlobs::new_for_testing(
            signed,
        ))
    } else if let Ok(tx) = TxLegacy::decode(&mut &payload[..]) {
        let tx = reth::primitives::Transaction::from(tx);
        let signed = dummy_signer.sign_tx(tx)?;
        Ok(TransactionSignedEcRecoveredWithBlobs::new_for_testing(
            signed,
        ))
    } else {
        eyre::bail!("Unsupported tx format")
    }
}

async fn send_order(
    sender: &Arc<TokioMutex<Option<tokio::sync::mpsc::Sender<ReplaceableOrderPoolCommand>>>>,
    order: Order,
) -> eyre::Result<()> {
    let cmd = ReplaceableOrderPoolCommand::Order(order);
    if let Some(tx) = sender.lock().await.as_ref().cloned() {
        let _ = tx.send(cmd).await; // drop errors on shutdown/backpressure
    }
    Ok(())
}

impl ServoTx {
    fn sender(&self) -> Option<Address> {
        self.tx
            .get("from")
            .and_then(|v| v.as_str())
            .and_then(|s| Address::from_str(s).ok())
    }
}
