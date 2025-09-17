use std::sync::Arc;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use alloy_primitives::U256;

use crate::eureka::servo::servo_bid_coordinator::ServoBidCoordinator;
use crate::live_builder::block_output::bidding::interfaces::{
    BidMaker, BiddingService, BiddingServiceWinControl, BlockBidWithStatsObs, LandedBlockInfo,
    SlotBlockId,
};

#[derive(Debug)]
pub struct EnrichingBiddingService {
    primary: Arc<dyn BiddingService>,
    coordinator: ServoBidCoordinator,
}

impl EnrichingBiddingService {
    pub fn new(primary: Arc<dyn BiddingService>) -> Self {
        let margin = U256::from(5_000_000_000_000_000u64);
        Self {
            primary,
            coordinator: ServoBidCoordinator::new(margin),
        }
    }
}

impl BlockBidWithStatsObs for EnrichingBiddingService {
    fn update_new_bid(
        &self,
        bid: crate::live_builder::block_output::bidding::block_bid_with_stats::BlockBidWithStats,
    ) {
        self.coordinator.update_competition_bid(
            bid.bid.block_number,
            bid.bid.slot_number,
            bid.bid.value,
        );
        // forward to primary (it also observes bids)
        self.primary.update_new_bid(bid);
    }
}

impl BiddingService for EnrichingBiddingService {
    fn create_slot_bidder(
        &self,
        slot_block_id: SlotBlockId,
        slot_timestamp: OffsetDateTime,
        bid_maker: Box<dyn BidMaker + Send + Sync>,
        cancel: CancellationToken,
    ) -> Arc<dyn crate::building::builders::UnfinishedBlockBuildingSink> {
        // No wrapping required; forward to primary service as-is
        self.primary
            .create_slot_bidder(slot_block_id, slot_timestamp, bid_maker, cancel)
    }

    fn win_control(&self) -> Arc<dyn BiddingServiceWinControl> {
        self.primary.win_control()
    }
    fn update_new_landed_blocks_detected(&self, landed: &[LandedBlockInfo]) {
        self.primary.update_new_landed_blocks_detected(landed)
    }
    fn update_failed_reading_new_landed_blocks(&self) {
        self.primary.update_failed_reading_new_landed_blocks()
    }
}
