# SERVO Migration Plan (ebuilder → rbuilder via bid-scraper)

Goal: Port the SERVO integration from ebuilder to upstream rbuilder while replacing the in-process Ultrasound WebSocket with bid-scraper’s NNG feed. Keep changes minimal and localized in rbuilder; do not reintroduce a custom WS client.

## Current vs. Target

- ebuilder (current):
  - Competition source is an in-process Ultrasound WS (SSZ decode + observers) exposed as `CompetitionBidProvider`.
  - SERVO logic pulls latest bids via `latest_bid(block, slot)` and subscribes for updates.

- rbuilder (target):
  - Competition data is externalized via bid-scraper (NNG PUB/SUB). rbuilder should subscribe to `scraped_bids_publisher_url`, cache latest top bids by `(block, slot)`, and expose the same `CompetitionBidProvider` surface.

## Architecture change summary

- Add a new `CompetitionBidProvider` implementation that subscribes to bid-scraper’s NNG stream and maintains a small in-process cache keyed by `(block_number, slot_number)`.
- Wire this provider into live-builder construction when `scraped_bids_publisher_url` is configured.
- SERVO coordinator code continues to read `latest_bid()` and subscribe as before; no WS logic in rbuilder.

## Files to add/edit (upstream rbuilder)

- Add: `crates/rbuilder/src/live_builder/block_output/bid_value_source/scraped_nng_source.rs`
  - Implements a `BidValueSource` + `CompetitionBidProvider` backed by NNG feed:
    - Starts `bid_scraper::bid_scraper_client::run_nng_subscriber_with_retries` with the configured `publisher_url`.
    - On each `BlockBid` that represents a top bid (e.g., `publisher_type.publishes_only_top_bid()`), updates an internal map: `HashMap<(block, slot), (U256, Instant)>`.
    - `latest_bid(block, slot) -> Option<U256>` returns current cached value.
    - `subscribe_competition(block, slot, obs)` registers observers and emits updates when cache changes.

- Edit: `crates/rbuilder/src/live_builder/config.rs`
  - L1Config (existing field): `scraped_bids_publisher_url: Option<String>` is already present.
  - Optional (future): `scraped_bids_cache_ttl_secs: Option<u64> = Some(32)` (only if you plan to add cleanup loop later).
  - During builder creation (same place that constructs sink and WalletBalanceWatcher), instantiate `ScrapedNngBidValueSource` if `scraped_bids_publisher_url` is set and pass it to downstream components that need `Arc<dyn CompetitionBidProvider>` (e.g., SERVO startup). Reuse the existing timeouts in this module (e.g., `BID_SOURCE_TIMEOUT_SECS`, `BID_SOURCE_WAIT_TIME_SECS`).

- No change required in bid-scraper; Ultrasound publishers already exist:
  - `crates/bid-scraper/src/ultrasound_ws_publisher.rs`
  - `crates/bid-scraper/config.toml`

## Implementation sketch: `scraped_nng_source.rs`

```rust
use crate::live_builder::block_output::bid_value_source::interfaces::*;
use bid_scraper::{
    bid_scraper_client::{run_nng_subscriber_with_retries, ScrapedBidsObs},
    types::{BlockBid, PublisherType},
};
use alloy_primitives::U256;
use parking_lot::RwLock;
use std::{collections::HashMap, sync::{Arc, Weak}, time::{Duration, Instant}};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub struct ScrapedNngBidValueSource {
    cache: Arc<RwLock<HashMap<(u64, u64), (U256, Instant)>>>,
    subs: Arc<RwLock<HashMap<(u64, u64), Vec<Arc<dyn BidValueObs>>>>>,
    weak_subs: Arc<RwLock<HashMap<(u64, u64), Vec<Weak<dyn BidValueObs>>>>>,
    _task: JoinHandle<()>,
}

impl ScrapedNngBidValueSource {
    pub fn new(publisher_url: String, cancel: CancellationToken, timeout: Duration, retry_wait: Duration) -> Self {
        let cache = Arc::new(RwLock::new(HashMap::new()));
        let subs = Arc::new(RwLock::new(HashMap::new()));
        let weak_subs = Arc::new(RwLock::new(HashMap::new()));

        struct ObsImpl {
            cache: Arc<RwLock<HashMap<(u64, u64), (U256, Instant)>>>,
            subs: Arc<RwLock<HashMap<(u64, u64), Vec<Arc<dyn BidValueObs>>>>>,
            weak_subs: Arc<RwLock<HashMap<(u64, u64), Vec<Weak<dyn BidValueObs>>>>>,
        }
        impl ScrapedBidsObs for ObsImpl {
            fn update_new_bid(&self, bid: BlockBid) {
                // Only sources that publish current top bid
                if !bid.publisher_type.publishes_only_top_bid() { return; }
                let key = (bid.block_number, bid.slot_number);
                let value = bid.value;
                let mut guard = self.cache.write();
                let should_update = match guard.get(&key) { None => true, Some((prev, _)) if *prev < value => true, _ => false };
                if should_update {
                    guard.insert(key, (value, Instant::now()));
                    drop(guard);
                    // Notify strong + upgraded weak observers
                    let mut to_notify: Vec<Arc<dyn BidValueObs>> = Vec::new();
                    to_notify.extend(self.subs.read().get(&key).cloned().unwrap_or_default());
                    if let Some(weaks) = self.weak_subs.read().get(&key) {
                        to_notify.extend(weaks.iter().filter_map(|w| w.upgrade()));
                    }
                    for obs in to_notify { obs.update_new_bid(CompetitionBid::new(value)); }
                }
            }
        }

        let obs = Arc::new(ObsImpl { cache: cache.clone(), subs: subs.clone(), weak_subs: weak_subs.clone() });
        let _task = tokio::spawn(async move { run_nng_subscriber_with_retries(obs, cancel, publisher_url, timeout, retry_wait).await; });
        Self { cache, subs, weak_subs, _task }
    }
}

impl BidValueSource for ScrapedNngBidValueSource {
    fn subscribe(&self, block: u64, slot: u64, obs: Arc<dyn BidValueObs>) {
        self.subs.write().entry((block, slot)).or_default().push(obs);
    }
    fn unsubscribe(&self, _obs: Arc<dyn BidValueObs>) { /* no-op */ }
}

impl CompetitionBidProvider for ScrapedNngBidValueSource {
    fn latest_bid(&self, block: u64, slot: u64) -> Option<U256> {
        self.cache.read().get(&(block, slot)).map(|(v, _)| *v)
    }
    fn subscribe_competition(&self, b: u64, s: u64, o: Arc<dyn BidValueObs>) { self.subscribe(b, s, o) }
}
```

## Wiring in `live_builder/config.rs`

- When creating the sealed sink factory and other `LiveBuilder` dependencies, build the `competition_provider` as:

```rust
let competition_provider: Arc<dyn CompetitionBidProvider> = if let Some(url) = l1_config.scraped_bids_publisher_url.clone() {
    let cancel = cancellation_token.child_token();
    let src = ScrapedNngBidValueSource::new(
        url,
        cancel,
        Duration::from_secs(BID_SOURCE_TIMEOUT_SECS),
        Duration::from_secs(BID_SOURCE_WAIT_TIME_SECS),
    );
    Arc::new(src)
} else {
    Arc::new(NullBidValueSource {})
};
```

- Pass `competition_provider` to any components that need `Arc<dyn CompetitionBidProvider>` (e.g., SERVO startup).

## Configuration

- bid-scraper should be running with Ultrasound WS publishers (EU/US) and publishing to an NNG endpoint:
  - See: `crates/bid-scraper/config.toml`.
- rbuilder should subscribe via:

```toml
# In config-live-example.toml (or your env-specific config)
scraped_bids_publisher_url = "tcp://0.0.0.0:5555"
```

## Refactoring step: standardise on alloy and native WS

- WebSockets in rbuilder internal code:
  - Replace tokio-tungstenite usages in rbuilder (e.g., SERVO allocation client) with `tokio-websockets` on top of tokio and rustls. Keep bid-scraper’s WS as-is (out of process).
  - Update message handling (`Message::Ping/Pong/Binary/Text`) accordingly; remove `MaybeTlsStream`/split sink/stream types.
  - Pin crate versions and features to match staging servers (enable `rustls-tls`, add `deflate` if compression is used).

- Providers and primitives:
  - Use `alloy_primitives::U256` (and friends) uniformly; remove residual `primitive_types`/`ethers_*` imports.
  - Ensure `ServoBidCoordinator` and helpers construct providers via `alloy_provider` (e.g., `alloy_provider::Http`).
  - Avoid duplicating SSZ structs that already exist in bid-scraper; re-export them from a glue module if needed.

- Build policy:
  - Keep `tokio-tungstenite` feature-gated only under bid-scraper; deny its usage elsewhere in rbuilder.

## Corner cases and mitigations

- Competition-bid ingestion:
  - Burst traffic may cause lock contention: consider `DashMap` or sharded maps if needed.
  - Duplicate publishers/relays: filter by `publishes_only_top_bid()`; optionally add a config to accept duplicates.
  - Reconnect backoff: increase `retry_wait` exponentially on persistent failures.

- SERVO decision flow:
  - Stalled feeds: rely on per-slot cache + TTL; skip bids when no fresh data.
  - Rapid bid raises: subscription path must update cache before returning to avoid stale reads.
  - Alloy version drift: pin alloy minor version; update profit tests on upgrade.

## Acceptance criteria

1. `cargo clippy -D warnings` and tests pass.
2. With bid-scraper active, SERVO coordinator logs show a non-None competition snapshot during evaluation.
3. On feed stalls beyond timeout, the NNG subscriber reconnects; coordinator falls back safely.
4. Shutdown via `CancellationToken` cancels the NNG task cleanly (no panics, sockets close).
5. No rbuilder-internal dependency on tokio-tungstenite remains; WebSocket client uses `tokio-websockets`.
6. All providers and primitives standardised on alloy types; no stray `primitive_types`/`ethers_*` in rbuilder.

