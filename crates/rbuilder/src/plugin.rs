//! Core plugin traits and a simple registry.
//! Compiled only when the `plugins` feature is enabled.

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Every plugin provides a stable, static name.
pub trait Named {
    fn name(&self) -> &'static str;
}

/// Hooks around order input wiring/spawn.
#[async_trait]
pub trait OrderInputHook: Named + Send + Sync {
    /// Called once when order input subsystem is started.
    /// Implementations should observe `cancel` and return promptly on shutdown.
    async fn on_order_input_started(&self, cancel: CancellationToken) -> eyre::Result<()>;
}

/// Hooks for external bid feeds or observers.
pub trait BidFeedHook: Named + Send + Sync {
    /// Start the external feed; should return immediately if setup is async-spawned.
    fn start(&self, cancel: CancellationToken) -> eyre::Result<()>;
}

/// Hooks for lifecycle events of the live builder process.
pub trait LifecycleHook: Named + Send + Sync {
    /// Called when the live builder run begins.
    fn on_builder_started(&self) {}
    /// Called when the live builder is shutting down.
    fn on_builder_stopped(&self) {}
}

/// Optional RPC extension hook. Implementations can extend the JSON-RPC module.
#[cfg(feature = "plugins")]
pub trait RpcHook: Named + Send + Sync {
    type Ctx;
    fn extend(&self, module: jsonrpsee::RpcModule<Self::Ctx>) -> eyre::Result<jsonrpsee::RpcModule<Self::Ctx>>;
}

/// Simple, typed plugin registry.
/// Stores typed collections without any type erasure/downcasting.
#[derive(Default, Debug)]
pub struct PluginRegistry {
    order_input_hooks: Vec<Arc<dyn OrderInputHook>>, // plugins registry
    bid_feed_hooks: Vec<Arc<dyn BidFeedHook>>,       // plugins registry
    lifecycle_hooks: Vec<Arc<dyn LifecycleHook>>,    // plugins registry
}

impl PluginRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn register_order_input_hook(&mut self, hook: Arc<dyn OrderInputHook>) { self.order_input_hooks.push(hook); }
    pub fn register_bid_feed_hook(&mut self, hook: Arc<dyn BidFeedHook>) { self.bid_feed_hooks.push(hook); }
    pub fn register_lifecycle_hook(&mut self, hook: Arc<dyn LifecycleHook>) { self.lifecycle_hooks.push(hook); }

    pub fn order_input_hooks(&self) -> &[Arc<dyn OrderInputHook>] { &self.order_input_hooks }
    pub fn lifecycle_hooks(&self) -> &[Arc<dyn LifecycleHook>] { &self.lifecycle_hooks }
    pub fn bid_feed_hooks(&self) -> &[Arc<dyn BidFeedHook>] { &self.bid_feed_hooks }
}
