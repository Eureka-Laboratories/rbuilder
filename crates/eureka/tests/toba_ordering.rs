use std::sync::Arc;

use alloy_primitives::U256;
use rbuilder::building::block_orders::PrioritizedOrderStore;
use rbuilder::building::block_orders::SimulatedOrderSink;
use rbuilder::building::order_priority::{
    FullProfitInfoGetter, OrderMaxProfitPriority, TobaAwareOrderPriority,
};
use rbuilder::primitives::{AccountNonce, Order, SimValue, SimulatedOrder, TestDataGenerator};

// Unit-style: TOBA wrapper should rank TOBA > non-TOBA even if underlying priority prefers otherwise.
#[test]
fn toba_is_prioritized_over_non_toba() {
    let mut gen = TestDataGenerator::default();

    // Non-TOBA mempool tx with higher profit
    let non_toba_addr = gen.base.create_address();
    let non_toba_order = gen.create_tx_order(AccountNonce {
        account: non_toba_addr,
        nonce: 0,
    });
    let non_toba = Arc::new(SimulatedOrder {
        order: non_toba_order,
        sim_value: SimValue::new_test(U256::from(10_000), U256::from(10_000), 100),
        used_state_trace: None,
    });

    // TOBA mempool tx with lower profit
    let toba_addr = gen.base.create_address();
    let mut toba_order = gen.create_tx_order(AccountNonce {
        account: toba_addr,
        nonce: 0,
    });
    if let Order::Tx(tx) = &mut toba_order {
        tx.tx_with_blobs.metadata.is_toba = true;
    }
    let toba = Arc::new(SimulatedOrder {
        order: toba_order,
        sim_value: SimValue::new_test(U256::from(1_000), U256::from(1_000), 100),
        used_state_trace: None,
    });

    type P = TobaAwareOrderPriority<OrderMaxProfitPriority<FullProfitInfoGetter>>;
    let lhs =
        TobaAwareOrderPriority::<OrderMaxProfitPriority<FullProfitInfoGetter>>::new_with_order(
            toba,
        );
    let rhs =
        TobaAwareOrderPriority::<OrderMaxProfitPriority<FullProfitInfoGetter>>::new_with_order(
            non_toba,
        );

    assert!(lhs > rhs, "TOBA should be prioritized over non-TOBA");
}

// E2E-style (ordering flow): when both orders are ready, the first popped order is TOBA.
#[test]
fn prioritized_store_pops_toba_first() {
    let mut gen = TestDataGenerator::default();

    // Build a TOBA mempool tx
    let toba_addr = gen.base.create_address();
    let mut toba_order = gen.create_tx_order(AccountNonce {
        account: toba_addr,
        nonce: 0,
    });
    if let Order::Tx(tx) = &mut toba_order {
        tx.tx_with_blobs.metadata.is_toba = true;
    }
    let toba_sim = Arc::new(SimulatedOrder {
        order: toba_order,
        sim_value: SimValue::new_test(U256::from(1_000), U256::from(1_000), 100),
        used_state_trace: None,
    });

    // Build a non-TOBA higher-profit mempool tx
    let nt_addr = gen.base.create_address();
    let non_toba_order = gen.create_tx_order(AccountNonce {
        account: nt_addr,
        nonce: 0,
    });
    let non_toba_sim = Arc::new(SimulatedOrder {
        order: non_toba_order,
        sim_value: SimValue::new_test(U256::from(10_000), U256::from(10_000), 100),
        used_state_trace: None,
    });

    let mut store: PrioritizedOrderStore<
        TobaAwareOrderPriority<OrderMaxProfitPriority<FullProfitInfoGetter>>,
    > = PrioritizedOrderStore::new([]);

    store.insert_order(toba_sim.clone());
    store.insert_order(non_toba_sim.clone());

    let first = store.pop_order().expect("first order present");
    assert!(first.order.is_toba(), "TOBA order should be popped first");
}
