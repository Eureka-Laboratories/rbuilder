use std::sync::Arc;

use alloy_primitives::U256;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use crate::building::builders::{
    block_building_helper::BiddableUnfinishedBlock, UnfinishedBlockBuildingSink,
};
use crate::live_builder::block_output::bidding::block_bid_with_stats::BlockBidWithStats;
use crate::live_builder::block_output::bidding::interfaces::{
    Bid, BidMaker, BiddingService, BiddingServiceWinControl, BlockBidWithStatsObs, SlotBlockId,
};

use crate::eureka::servo::servo_bid_coordinator::{self, ServoBidCoordinator};

#[derive(Debug, Default)]
pub struct ServoBiddingService {
    coordinator: ServoBidCoordinator,
}

impl ServoBiddingService {
    pub fn new(
        _landed_blocks: &[crate::live_builder::block_output::bidding::interfaces::LandedBlockInfo],
    ) -> Self {
        // default 0.005 ETH margin
        let margin = U256::from(5_000_000_000_000_000u64);
        let svc = Self {
            coordinator: ServoBidCoordinator::new(margin),
        };
        // Expose globally so HTTP bidder can read competition via shared coordinator
        servo_bid_coordinator::set_global(Arc::new(svc.coordinator.clone()));
        svc
    }
}

impl BlockBidWithStatsObs for ServoBiddingService {
    fn update_new_bid(&self, bid_with_stats: BlockBidWithStats) {
        self.coordinator.update_competition_bid(
            bid_with_stats.bid.block_number,
            bid_with_stats.bid.slot_number,
            bid_with_stats.bid.value,
        );
    }
}

impl BiddingService for ServoBiddingService {
    fn create_slot_bidder(
        &self,
        slot_block_id: SlotBlockId,
        _slot_timestamp: OffsetDateTime,
        bid_maker: Box<dyn BidMaker + Send + Sync>,
        _cancel: CancellationToken,
    ) -> Arc<dyn UnfinishedBlockBuildingSink> {
        Arc::new(ServoSlotBidder {
            bid_maker,
            coordinator: self.coordinator.clone(),
            slot: slot_block_id.slot(),
            block: slot_block_id.block(),
        })
    }

    fn win_control(&self) -> Arc<dyn BiddingServiceWinControl> {
        Arc::new(NoopWinControl {})
    }

    fn update_new_landed_blocks_detected(
        &self,
        _landed_blocks: &[crate::live_builder::block_output::bidding::interfaces::LandedBlockInfo],
    ) {
    }
    fn update_failed_reading_new_landed_blocks(&self) {}
}

#[derive(Debug)]
struct ServoSlotBidder {
    bid_maker: Box<dyn BidMaker + Send + Sync>,
    coordinator: ServoBidCoordinator,
    slot: u64,
    block: u64,
}

impl UnfinishedBlockBuildingSink for ServoSlotBidder {
    fn new_block(&self, block: BiddableUnfinishedBlock) {
        // Evaluate decision based on current competition snapshot
        let tbv = block.true_block_value();
        let competition = self.coordinator.latest_bid(self.block, self.slot);
        let should = self
            .coordinator
            .should_bid_against_competition(tbv, U256::ZERO, competition);
        if !should {
            return;
        }

        let payout_tx_value = if block.can_add_payout_tx() {
            Some(tbv)
        } else {
            None
        };
        self.bid_maker
            .send_bid(Bid::new(block, payout_tx_value, competition));
    }

    fn can_use_suggested_fee_recipient_as_coinbase(&self) -> bool {
        false
    }
}

#[derive(Debug)]
struct NoopWinControl {}
impl BiddingServiceWinControl for NoopWinControl {
    fn must_win_block(&self, _block: u64) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    // no-op imports in minimal test

    #[test]
    fn updates_latest_and_creates_sink() {
        let svc = ServoBiddingService::new(&[]);
        // update via bid sink
        let bb = BlockBidWithStats::new(bid_scraper::types::BlockBid {
            seen_time: 0.0,
            publisher_name: "x".into(),
            publisher_type: bid_scraper::types::PublisherType::UltrasoundWs,
            relay_time: None,
            relay_name: "x".into(),
            block_hash: alloy_primitives::BlockHash::ZERO,
            parent_hash: alloy_primitives::BlockHash::ZERO,
            value: U256::from(42),
            slot_number: 1,
            block_number: 2,
            builder_pubkey: None,
            extra_data: None,
            fee_recipient: None,
            proposer_fee_recipient: None,
            gas_used: None,
            optimistic_submission: None,
        });
        BlockBidWithStatsObs::update_new_bid(&svc, bb);
        // create sink
        let sink = BiddingService::create_slot_bidder(
            &svc,
            SlotBlockId::new(0, 0, alloy_primitives::BlockHash::ZERO),
            OffsetDateTime::now_utc(),
            Box::new(TestBidMaker),
            CancellationToken::new(),
        );
        // minimal assertion: sink exists and flags
        assert!(!sink.can_use_suggested_fee_recipient_as_coinbase());
    }

    #[derive(Debug)]
    struct TestBidMaker;
    impl BidMaker for TestBidMaker {
        fn send_bid(&self, _bid: Bid) { /* ok */
        }
    }
}
