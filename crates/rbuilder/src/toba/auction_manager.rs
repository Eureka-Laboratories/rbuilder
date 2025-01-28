use crate::toba::TopBlockTx;
use tokio::sync::{Mutex, Notify};
use tracing::info;

#[derive(Debug)]
pub struct TopBlockAuctionManager {
    candidate: Mutex<Option<TopBlockTx>>,
    notify: Notify,
}
impl TopBlockAuctionManager {
    pub fn new() -> Self {
        info!("I'd like to speak to the manager");
        Self {
            candidate: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    pub async fn get_current_candidate(&self) -> Option<TopBlockTx> {
        self.candidate.lock().await.clone()
    }

    pub async fn set_candidate(&self, tx: TopBlockTx) {
        let mut guard = self.candidate.lock().await;
        *guard = Some(tx);
        self.notify.notify_one();
    }

    pub async fn clear_candidate(&self) {
        let mut guard = self.candidate.lock().await;
        *guard = None;
        self.notify.notify_one();
    }
}
