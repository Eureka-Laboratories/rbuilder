use crate::live_builder::order_input::ReplaceableOrderPoolCommand;
use crate::primitives::serialize::{RawTx, TxEncoding};
use crate::primitives::{MempoolTx, Order};
use futures::StreamExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};

pub async fn start_ws_server(
    results: mpsc::Sender<ReplaceableOrderPoolCommand>,
    global_cancel: CancellationToken,
) -> eyre::Result<JoinHandle<()>> {
    let addr = "127.0.0.1:3030";
    let listener = TcpListener::bind(&addr).await?;
    info!("WebSocket server listening on: ws://{}", addr);

    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                Ok((stream, addr)) = listener.accept() => {
                    info!("New connection from: {}", addr);
                    let results = results.clone();
                    tokio::spawn(handle_connection(stream, results));
                },
                _ = global_cancel.cancelled() => {
                    info!("WS server is shutting down");
                    break;
                },
            }
        }
    });

    Ok(handle)
}


async fn handle_connection(
    stream: tokio::net::TcpStream,
    results: mpsc::Sender<ReplaceableOrderPoolCommand>,
) {
    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .expect("Error during the websocket handshake");

    let (_, read) = ws_stream.split();

    read.for_each(|message| async {
        match message {
            Ok(msg) => match msg {
                Message::Text(text) => {
                    let raw_tx_order: RawTx = serde_json::from_str(&text).expect("Failed to deserialize RawTx");
                    let tx: MempoolTx = match raw_tx_order.decode(TxEncoding::WithBlobData) {
                        Ok(tx) => { tx }
                        Err(err) => {
                            warn!(?err, "Failed to decode raw transaction");
                            return;
                        }
                    };

                    let order = Order::Tx(tx);
                    info!(order = ?order.id(), "Received mempool tx from WebSocket");

                    if let Err(e) = results
                        .send(ReplaceableOrderPoolCommand::Order(order))
                        .await
                    {
                        error!("Failed to send command: {}", e);
                    }
                }
                Message::Close(_) => {
                    info!("Client disconnected");
                }
                Message::Ping(_) => {
                    trace!("Ping");
                }
                e => {
                    warn!("Received unsupported message type {}", e);
                }
            },
            Err(e) => {
                error!("Error processing WebSocket message: {}", e);
            }
        }
    })
        .await;
}
