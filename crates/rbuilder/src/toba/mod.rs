pub mod auction_manager;
pub(crate) mod ws_server;

use crate::primitives::TransactionSignedEcRecoveredWithBlobs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct TopBlockTx {
    pub transaction: TransactionSignedEcRecoveredWithBlobs,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BidAction {
    SubmitBid { transaction: String },
    CancelBid { bid_id: String },
}
