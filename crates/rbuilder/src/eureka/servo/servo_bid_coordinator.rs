use alloy_primitives::U256;
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
        let e = map.entry((block, slot)).or_insert(U256::ZERO);
        if value > *e {
            *e = value;
        }
    }

    pub fn latest_bid(&self, block: u64, slot: u64) -> Option<U256> {
        self.latest.read().unwrap().get(&(block, slot)).copied()
    }

    pub fn clear_slot_cache(&self, block: u64, slot: u64) {
        self.latest.write().unwrap().remove(&(block, slot));
    }

    /// Minimal evaluation: decide to bid when base+servo_total >= competition + margin
    pub fn should_bid_against_competition(
        &self,
        base_block_value: U256,
        servo_total_value: U256,
        competition: Option<U256>,
    ) -> bool {
        match competition {
            Some(c) => {
                base_block_value.saturating_add(servo_total_value)
                    >= c.saturating_add(self.min_outbid_margin)
            }
            None => false,
        }
    }
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
}
