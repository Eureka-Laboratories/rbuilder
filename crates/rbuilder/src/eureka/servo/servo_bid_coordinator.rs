use alloy_primitives::U256;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Default, Clone)]
pub struct ServoBidCoordinator {
    latest: Arc<RwLock<HashMap<(u64, u64), U256>>>,
    // Simple margin for tests (wei)
    min_outbid_margin: U256,
}

impl ServoBidCoordinator {
    pub fn new(min_outbid_margin: U256) -> Self {
        Self {
            latest: Arc::new(RwLock::new(HashMap::new())),
            min_outbid_margin,
        }
    }

    pub fn update_competition_bid(&self, block: u64, slot: u64, value: U256) {
        let mut map = self.latest.write().unwrap();
        let prev = map.get(&(block, slot)).copied().unwrap_or_default();
        if value > prev {
            map.insert((block, slot), value);
            tracing::debug!(
                block,
                slot,
                value = %value,
                "SERVO: updated competition bid snapshot"
            );
        }
    }

    pub fn latest_bid(&self, block: u64, slot: u64) -> Option<U256> {
        let val = self.latest.read().unwrap().get(&(block, slot)).copied();
        if let Some(v) = val {
            tracing::trace!(block, slot, value = %v, "SERVO: latest competition bid");
        }
        val
    }

    pub fn clear_slot_cache(&self, block: u64, slot: u64) {
        let removed = self.latest.write().unwrap().remove(&(block, slot));
        if removed.is_some() {
            tracing::debug!(block, slot, "SERVO: cleared per-slot competition cache");
        }
    }

    /// Minimal evaluation: decide to bid when base+servo_total >= competition + margin
    pub fn should_bid_against_competition(
        &self,
        base_block_value: U256,
        servo_total_value: U256,
        competition: Option<U256>,
    ) -> bool {
        let should = match competition {
            Some(c) => {
                base_block_value.saturating_add(servo_total_value)
                    >= c.saturating_add(self.min_outbid_margin)
            }
            None => false,
        };
        tracing::debug!(
            base_block_value = %base_block_value,
            servo_total_value = %servo_total_value,
            competition = %competition.unwrap_or_default(),
            min_outbid_margin = %self.min_outbid_margin,
            should,
            "SERVO: evaluation against competition"
        );
        should
    }
}

// Global coordinator accessor to share the same instance between NNG feed and HTTP bidder.
lazy_static! {
    static ref GLOBAL_COORDINATOR: RwLock<Option<Arc<ServoBidCoordinator>>> = RwLock::new(None);
}

pub fn set_global(coordinator: Arc<ServoBidCoordinator>) {
    let mut guard = GLOBAL_COORDINATOR.write().unwrap();
    *guard = Some(coordinator);
}

pub fn global() -> Option<Arc<ServoBidCoordinator>> {
    GLOBAL_COORDINATOR.read().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_reads_latest() {
        let coord = ServoBidCoordinator::new(U256::from(1));
        coord.update_competition_bid(10, 20, U256::from(100));
        assert_eq!(coord.latest_bid(10, 20), Some(U256::from(100)));
        // smaller update ignored
        coord.update_competition_bid(10, 20, U256::from(50));
        assert_eq!(coord.latest_bid(10, 20), Some(U256::from(100)));
        coord.clear_slot_cache(10, 20);
        assert_eq!(coord.latest_bid(10, 20), None);
    }

    #[test]
    fn simple_margin_evaluation() {
        let coord = ServoBidCoordinator::new(U256::from(10)); // 10 wei margin
        let base = U256::from(50);
        let servo = U256::from(60); // total 110
        assert!(coord.should_bid_against_competition(base, servo, Some(U256::from(99)))); // 99+10=109
        assert!(!coord.should_bid_against_competition(base, servo, Some(U256::from(101)))); // 101+10=111
        assert!(!coord.should_bid_against_competition(base, servo, None));
    }

    #[test]
    fn global_set_and_get() {
        let coord = Arc::new(ServoBidCoordinator::new(U256::from(1)));
        super::set_global(coord.clone());
        let got = super::global().unwrap();
        assert!(Arc::ptr_eq(&coord, &got));
    }
}
