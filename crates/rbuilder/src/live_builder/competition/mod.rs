use alloy_primitives::U256;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// Minimal observer interface used by Servo for competition bid updates.
pub trait BidValueObs: Send + Sync + std::fmt::Debug {
    fn update_new_bid(&self, bid: CompetitionBid);
}

#[derive(Debug, Clone, Copy)]
pub struct CompetitionBid(U256);
impl CompetitionBid {
    pub fn new(v: U256) -> Self { Self(v) }
    pub fn bid(&self) -> U256 { self.0 }
}

// Minimal provider surface needed by Servo logic.
pub trait CompetitionBidProvider: Send + Sync + std::fmt::Debug {
    fn latest_bid(&self, block_number: u64, slot_number: u64) -> Option<U256>;
    fn subscribe_competition(&self, block_number: u64, slot_number: u64, obs: Arc<dyn BidValueObs>);
}

// NNG-backed provider ingesting bids published by bid-scraper. Not wired yet.
#[derive(Debug)]
pub struct ScrapedNngBidValueSource {
    cache: Arc<RwLock<HashMap<(u64, u64), (U256, Instant)>>>,
    strong_subs: Arc<RwLock<HashMap<(u64, u64), Vec<Arc<dyn BidValueObs>>>>>,
    weak_subs: Arc<RwLock<HashMap<(u64, u64), Vec<Weak<dyn BidValueObs>>>>>,
    _task: JoinHandle<()>,
}

impl ScrapedNngBidValueSource {
    pub fn new(
        publisher_url: String,
        cancel: CancellationToken,
        timeout: Duration,
        retry_wait: Duration,
    ) -> Self {
        let cache = Arc::new(RwLock::new(HashMap::new()));
        let strong_subs = Arc::new(RwLock::new(HashMap::new()));
        let weak_subs = Arc::new(RwLock::new(HashMap::new()));

        // Forwarder from bid-scraper NNG subscriber
        struct ObsImpl {
            cache: Arc<RwLock<HashMap<(u64, u64), (U256, Instant)>>>,
            strong_subs: Arc<RwLock<HashMap<(u64, u64), Vec<Arc<dyn BidValueObs>>>>>,
            weak_subs: Arc<RwLock<HashMap<(u64, u64), Vec<Weak<dyn BidValueObs>>>>>,
        }
        impl bid_scraper::bid_scraper_client::ScrapedBidsObs for ObsImpl {
            fn update_new_bid(&self, bid: bid_scraper::types::BlockBid) {
                // accept only sources that publish current top bid to avoid duplicates
                if !bid.publisher_type.publishes_only_top_bid() { return; }
                let key = (bid.block_number, bid.slot_number);
                let val = bid.value;
                let mut guard = self.cache.write();
                let update = match guard.get(&key) {
                    None => true,
                    Some((prev, _)) if *prev < val => true,
                    _ => false,
                };
                if update {
                    guard.insert(key, (val, Instant::now()));
                    drop(guard);
                    // notify observers
                    let mut to_notify: Vec<Arc<dyn BidValueObs>> = Vec::new();
                    if let Some(v) = self.strong_subs.read().get(&key) { to_notify.extend(v.clone()); }
                    if let Some(v) = self.weak_subs.read().get(&key) {
                        to_notify.extend(v.iter().filter_map(|w| w.upgrade()));
                    }
                    let cb = CompetitionBid::new(val);
                    for obs in to_notify { obs.update_new_bid(cb); }
                }
            }
        }

        let obs = Arc::new(ObsImpl { cache: cache.clone(), strong_subs: strong_subs.clone(), weak_subs: weak_subs.clone() });
        let _task = tokio::spawn(async move {
            bid_scraper::bid_scraper_client::run_nng_subscriber_with_retries(
                obs,
                cancel,
                publisher_url,
                timeout,
                retry_wait,
            ).await;
        });

        Self { cache, strong_subs, weak_subs, _task }
    }

    #[cfg(test)]
    fn test_insert(&self, block: u64, slot: u64, val: U256) {
        self.cache.write().insert((block, slot), (val, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn latest_bid_returns_inserted_value() {
        let cancel = CancellationToken::new();
        let src = ScrapedNngBidValueSource::new(
            "tcp://127.0.0.1:0".to_string(),
            cancel,
            Duration::from_secs(1),
            Duration::from_secs(1),
        );
        src.test_insert(100, 2000, U256::from(1234u64));
        assert_eq!(src.latest_bid(100, 2000), Some(U256::from(1234u64)));
    }
}

impl CompetitionBidProvider for ScrapedNngBidValueSource {
    fn latest_bid(&self, block_number: u64, slot_number: u64) -> Option<U256> {
        self.cache.read().get(&(block_number, slot_number)).map(|(v, _)| *v)
    }
    fn subscribe_competition(&self, block_number: u64, slot_number: u64, obs: Arc<dyn BidValueObs>) {
        self.strong_subs
            .write()
            .entry((block_number, slot_number))
            .or_default()
            .push(obs);
    }
}
