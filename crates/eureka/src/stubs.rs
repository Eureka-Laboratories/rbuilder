use std::sync::Arc;

use async_trait::async_trait;
use eyre::Result;
use rbuilder::plugin::{BidFeedHook, LifecycleHook, Named, OrderInputHook, PluginRegistry};
use tokio_util::sync::CancellationToken;
use tracing::info;

pub struct NoopOrderInput;
impl Named for NoopOrderInput {
    fn name(&self) -> &'static str {
        "noop-order-input"
    }
}
#[async_trait]
impl OrderInputHook for NoopOrderInput {
    async fn on_order_input_started(&self, cancel: CancellationToken) -> Result<()> {
        info!(target: "eureka", name = self.name(), "Order input noop started");
        tokio::spawn(async move {
            cancel.cancelled().await;
        });
        Ok(())
    }
}

pub struct NoopBidFeed;
impl Named for NoopBidFeed {
    fn name(&self) -> &'static str {
        "noop-bid-feed"
    }
}
impl BidFeedHook for NoopBidFeed {
    fn start(&self, cancel: CancellationToken) -> Result<()> {
        info!(target: "eureka", name = self.name(), "Bid feed noop started");
        tokio::spawn(async move {
            cancel.cancelled().await;
        });
        Ok(())
    }
}

pub struct NoopLifecycle;
impl Named for NoopLifecycle {
    fn name(&self) -> &'static str {
        "noop-lifecycle"
    }
}
impl LifecycleHook for NoopLifecycle {}

pub fn register_noop_plugins(reg: &mut PluginRegistry) {
    reg.register_order_input_hook(Arc::new(NoopOrderInput));
    reg.register_bid_feed_hook(Arc::new(NoopBidFeed));
    reg.register_lifecycle_hook(Arc::new(NoopLifecycle));
}
