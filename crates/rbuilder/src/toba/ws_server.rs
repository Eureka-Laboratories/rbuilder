use crate::primitives::serialize::TxEncoding;
use crate::toba::auction_manager::TopBlockAuctionManager;
use crate::toba::{BidAction, TopBlockTx};
use futures::StreamExt;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};

pub async fn start_ws_server(
    manager: Arc<TopBlockAuctionManager>,
    addr: SocketAddr,
    cancel: CancellationToken,
) -> eyre::Result<JoinHandle<()>> {
    let listener = TcpListener::bind(&addr).await?;
    info!("TOBA WebSocket listening on: ws://{}", addr);

    async fn handle_connection(
        stream: tokio::net::TcpStream,
        manager: Arc<TopBlockAuctionManager>,
    ) {
        let ws_stream = tokio_tungstenite::accept_async(stream)
            .await
            .expect("Error during the websocket handshake");

        let (_, read) = ws_stream.split();

        read.for_each(|message| async {
            match message {
                Ok(msg) => match msg {
                    Message::Text(text) => match serde_json::from_str::<BidAction>(&text) {
                        Ok(bid_action) => match bid_action {
                            BidAction::SubmitBid { transaction } => {
                                if let Ok(bytes) = (&transaction).parse() {
                                    let tx = match TxEncoding::WithBlobData.decode(bytes) {
                                        Ok(tx) => tx,
                                        Err(err) => {
                                            warn!("Failed to decode transaction: {}", err);
                                            return;
                                        }
                                    };

                                    let top_block_tx = TopBlockTx { transaction: tx };
                                    manager.set_candidate(top_block_tx).await;
                                }
                            }
                            BidAction::CancelBid { .. } => {
                                todo!("Cancel bid");
                            }
                        },
                        Err(err) => {
                            warn!("Failed to parse BidAction: {}", err);
                        }
                    },
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

    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                Ok((stream, addr)) = listener.accept() => {
                    info!("New connection from: {}", addr);
                    let manager = manager.clone();
                    tokio::spawn(handle_connection(stream, manager));
                },
                _ = cancel.cancelled() => {
                    info!("WS server is shutting down");
                    break;
                },
            }
        }
    });

    Ok(handle)
}
