use futures::StreamExt;
use jsonrpsee::core::id_providers::RandomStringIdProvider;
use jsonrpsee::server::{RpcModule, Server};
use jsonrpsee::{PendingSubscriptionSink, SubscriptionMessage};
use serde_json::Value;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{info, warn};

// Function to create and start the JSON-RPC server
pub async fn start_block_subscription_server(
    ip: Ipv4Addr,
    port: u16,
) -> eyre::Result<broadcast::Sender<Value>> {
    let addr = SocketAddr::V4(SocketAddrV4::new(ip, port));
    let server = Server::builder()
        .set_message_buffer_capacity(5)
        .set_id_provider(RandomStringIdProvider::new(34))
        .build(addr)
        .await?;

    let (tx, _rx) = broadcast::channel::<Value>(16);
    let tx_clone = tx.clone();

    let mut module = RpcModule::new(tx_clone);

    module
        .register_subscription(
            "eth_subscribe",
            "eth_subscription",
            "eth_unsubscribe",
            |_params, pending, ctx| async move {
                let rx = ctx.subscribe();
                let stream = BroadcastStream::new(rx);
                pipe_from_stream(pending, stream).await?;
                Ok(())
            },
        )
        .unwrap();

    let addr = server.local_addr()?;
    let handle = server.start(module);

    tokio::spawn(handle.stopped());

    info!("Block subscription server started on {}", addr);
    info!("New WebSocket connection received");

    Ok(tx)
}

// Standalone function to handle sending messages from the broadcast stream to subscribers
async fn pipe_from_stream(
    pending: PendingSubscriptionSink,
    mut stream: BroadcastStream<Value>,
) -> Result<(), jsonrpsee::core::Error> {
    let sink = match pending.accept().await {
        Ok(sink) => sink,
        Err(e) => {
            warn!("Failed to accept subscription: {:?}", e);
            return Ok(());
        }
    };

    loop {
        tokio::select! {
            _ = sink.closed() => {
                // connection dropped
                break Ok(())
            },
            maybe_item = stream.next() => {
                let item = match maybe_item {
                    Some(Ok(item)) => item,
                    Some(Err(e)) => {
                        warn!("Error in WebSocket stream: {:?}", e);
                        break Ok(());
                    },
                    None => {
                        // stream ended
                        break Ok(())
                    },
                };
                let msg = SubscriptionMessage::from_json(&item)?;
                if sink.send(msg).await.is_err() {
                    break Ok(());
                }
            }
        }
    }
}
