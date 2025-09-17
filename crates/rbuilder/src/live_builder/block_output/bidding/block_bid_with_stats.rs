use std::sync::Arc;

use bid_scraper::{bid_scraper_client::ScrapedBidsObs, types::BlockBid};
use derivative::Derivative;
use time::OffsetDateTime;

use crate::{
    live_builder::block_output::bidding::interfaces::BlockBidWithStatsObs,
    telemetry::inc_bids_received,
};

/// BlockBid + extra info needed to measure bis travel times on the bidding service.
#[derive(Derivative, Clone, Debug)]
#[derivative(PartialEq, Eq)]
pub struct BlockBidWithStats {
    pub bid: BlockBid,
    /// Time this strucut was created, just before sending it to the bidding service
    #[derivative(PartialEq = "ignore")]
    creation_time: OffsetDateTime,
}

impl BlockBidWithStats {
    pub fn new(bid: BlockBid) -> Self {
        Self {
            bid,
            creation_time: OffsetDateTime::now_utc(),
        }
    }

    pub fn new_for_deserialization(bid: BlockBid, creation_time: OffsetDateTime) -> Self {
        Self { bid, creation_time }
    }

    pub fn creation_time(&self) -> OffsetDateTime {
        self.creation_time
    }
}

pub struct ScrapedBids2BlockBidWithStatsObs {
    obs: Arc<dyn BlockBidWithStatsObs>,
}

impl ScrapedBids2BlockBidWithStatsObs {
    pub fn new(obs: Arc<dyn BlockBidWithStatsObs>) -> Self {
        Self { obs }
    }
}

impl ScrapedBidsObs for ScrapedBids2BlockBidWithStatsObs {
    fn update_new_bid(&self, bid: BlockBid) {
        inc_bids_received(&bid);
        self.obs.update_new_bid(BlockBidWithStats::new(bid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, BlockHash, U256};
    use bid_scraper::types::PublisherType;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct TestSink {
        pub seen: Mutex<Vec<BlockBidWithStats>>,
    }
    impl BlockBidWithStatsObs for TestSink {
        fn update_new_bid(&self, bid_with_stats: BlockBidWithStats) {
            self.seen.lock().unwrap().push(bid_with_stats);
        }
    }

    #[test]
    fn forwards_block_bid_to_sink() {
        let sink = Arc::new(TestSink { seen: Mutex::new(Vec::new()) });
        let obs = ScrapedBids2BlockBidWithStatsObs::new(sink.clone());

        let bid = BlockBid {
            seen_time: 0.0,
            publisher_name: "ultrasound-eu".to_string(),
            publisher_type: PublisherType::UltrasoundWs,
            relay_time: Some(0.0),
            relay_name: "ultrasound-eu".to_string(),
            block_hash: BlockHash::ZERO,
            parent_hash: BlockHash::ZERO,
            value: U256::from(12345u64),
            slot_number: 123,
            block_number: 456,
            builder_pubkey: None,
            extra_data: None,
            fee_recipient: None,
            proposer_fee_recipient: None,
            gas_used: None,
            optimistic_submission: None,
        };

        obs.update_new_bid(bid);

        let got = sink.seen.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].bid.slot_number, 123);
        assert_eq!(got[0].bid.block_number, 456);
        assert_eq!(got[0].bid.value, U256::from(12345u64));
    }
}
