use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use alloy_primitives::U256;

use crate::building::builders::{
    block_building_helper::BiddableUnfinishedBlock,
    UnfinishedBlockBuildingSink,
};
use crate::live_builder::block_output::bidding::block_bid_with_stats::BlockBidWithStats;
use crate::live_builder::block_output::bidding::interfaces::{
    Bid, BidMaker, BiddingService, BiddingServiceWinControl, BlockBidWithStatsObs, LandedBlockInfo,
    SlotBlockId,
};

#[derive(Debug)]
pub struct MultiplexBiddingService {
    a: Arc<dyn BiddingService>,
    b: Arc<dyn BiddingService>,
}

impl MultiplexBiddingService {
    pub fn new(a: Arc<dyn BiddingService>, b: Arc<dyn BiddingService>) -> Self {
        Self { a, b }
    }
}

impl BlockBidWithStatsObs for MultiplexBiddingService {
    fn update_new_bid(&self, bid_with_stats: BlockBidWithStats) {
        self.a.update_new_bid(bid_with_stats.clone());
        self.b.update_new_bid(bid_with_stats);
    }
}

impl BiddingService for MultiplexBiddingService {
    fn create_slot_bidder(
        &self,
        slot_block_id: SlotBlockId,
        slot_timestamp: OffsetDateTime,
        bid_maker: Box<dyn BidMaker + Send + Sync>,
        cancel: CancellationToken,
    ) -> Arc<dyn UnfinishedBlockBuildingSink> {
        // Shared best-forwarding logic
        let best = Arc::new(Mutex::new(Option::<U256>::None));
        #[derive(Debug)]
        struct OuterMaker {
            inner: Mutex<Box<dyn BidMaker + Send + Sync>>,
        }
        impl BidMaker for OuterMaker {
            fn send_bid(&self, bid: Bid) {
                self.inner.lock().unwrap().send_bid(bid);
            }
        }
        let outer = Arc::new(OuterMaker { inner: Mutex::new(bid_maker) });
        #[derive(Debug)]
        struct Forward {
            best: Arc<Mutex<Option<U256>>>,
            outer: Arc<OuterMaker>,
        }
        impl BidMaker for Forward {
            fn send_bid(&self, bid: Bid) {
                let value = bid.payout_tx_value().unwrap_or(U256::ZERO);
                let mut guard = self.best.lock().unwrap();
                let should_forward = guard.map_or(true, |best_v| value > best_v);
                if should_forward {
                    *guard = Some(value);
                    self.outer.send_bid(bid);
                }
            }
        }
        let forwarder_a = Forward { best: best.clone(), outer: outer.clone() };
        let forwarder_b = Forward { best, outer };
        let sink_a = self.a.create_slot_bidder(
            slot_block_id.clone(),
            slot_timestamp,
            Box::new(forwarder_a),
            cancel.clone(),
        );
        let sink_b = self.b.create_slot_bidder(
            slot_block_id,
            slot_timestamp,
            Box::new(forwarder_b),
            cancel,
        );
        Arc::new(CompositeSlotSink { a: sink_a, b: sink_b })
    }

    fn win_control(&self) -> Arc<dyn BiddingServiceWinControl> {
        Arc::new(CompositeWinControl {
            a: self.a.win_control(),
            b: self.b.win_control(),
        })
    }

    fn update_new_landed_blocks_detected(&self, landed_blocks: &[LandedBlockInfo]) {
        self.a.update_new_landed_blocks_detected(landed_blocks);
        self.b.update_new_landed_blocks_detected(landed_blocks);
    }

    fn update_failed_reading_new_landed_blocks(&self) {
        self.a.update_failed_reading_new_landed_blocks();
        self.b.update_failed_reading_new_landed_blocks();
    }
}

#[derive(Debug)]
struct CompositeSlotSink {
    a: Arc<dyn UnfinishedBlockBuildingSink>,
    b: Arc<dyn UnfinishedBlockBuildingSink>,
}

impl UnfinishedBlockBuildingSink for CompositeSlotSink {
    fn new_block(&self, block: BiddableUnfinishedBlock) {
        self.a.new_block(block.clone());
        self.b.new_block(block);
    }

    fn can_use_suggested_fee_recipient_as_coinbase(&self) -> bool {
        self.a.can_use_suggested_fee_recipient_as_coinbase()
            || self.b.can_use_suggested_fee_recipient_as_coinbase()
    }
}

#[derive(Debug)]
struct CompositeWinControl {
    a: Arc<dyn BiddingServiceWinControl>,
    b: Arc<dyn BiddingServiceWinControl>,
}

impl BiddingServiceWinControl for CompositeWinControl {
    fn must_win_block(&self, block: u64) {
        self.a.must_win_block(block);
        self.b.must_win_block(block);
    }
}
