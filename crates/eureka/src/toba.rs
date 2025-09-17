use std::sync::Arc;

use alloy_primitives::Bytes;
use eyre::Result;
use jsonrpsee::{types::ErrorObject, RpcModule};
use rbuilder::live_builder::order_input::ReplaceableOrderPoolCommand;
use rbuilder::plugin::{Named, PluginRegistry, RpcDeps, RpcHook};
use rbuilder::primitives::{
    serialize::{RawTx, TxEncoding},
    MempoolTx, Order,
};
use tokio::sync::mpsc;
use tracing::{trace, warn};

pub struct TobaRpc;

impl Named for TobaRpc {
    fn name(&self) -> &'static str {
        "toba-rpc"
    }
}

impl RpcHook for TobaRpc {
    fn register(&self, module: &mut RpcModule<()>, deps: RpcDeps) -> Result<()> {
        let results_clone: mpsc::Sender<ReplaceableOrderPoolCommand> = deps.results.clone();
        let timeout = deps.timeout;
        module.register_async_method("eth_sendRawTransactionToba", move |params, _| {
            let results = results_clone.clone();
            async move {
                let raw_tx: Bytes = match params.one() {
                    Ok(raw_tx) => raw_tx,
                    Err(err) => {
                        warn!(?err, "TOBA: failed to parse raw tx");
                        return Err(err);
                    }
                };
                let raw_tx_order = RawTx { tx: raw_tx };
                // Decode to inner tx to set metadata
                let mut tx = match TxEncoding::WithBlobData.decode(raw_tx_order.tx.clone()) {
                    Ok(tx) => tx,
                    Err(err) => {
                        warn!(?err, "TOBA: failed to decode raw tx");
                        return Err(ErrorObject::owned(
                            -32602,
                            "failed to verify TOBA transaction",
                            None::<()>,
                        ));
                    }
                };
                tx.metadata.is_toba = true;
                let hash = tx.hash();
                let order = Order::Tx(MempoolTx::new(tx));
                trace!(order = ?order.id(), "Received TOBA tx from API");
                let _ = results
                    .send_timeout(ReplaceableOrderPoolCommand::Order(order), timeout)
                    .await;
                Ok(hash)
            }
        })?;
        Ok(())
    }
}

pub fn register_toba_hook(reg: &mut PluginRegistry) {
    reg.register_rpc_hook(Arc::new(TobaRpc));
}
