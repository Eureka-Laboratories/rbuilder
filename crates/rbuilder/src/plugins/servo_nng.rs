use crate::plugin::{BidFeedHook, Named};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct ServoNngBidFeedPlugin {
    enabled: bool,
}

impl ServoNngBidFeedPlugin {
    pub fn from_env() -> Self {
        let enabled = std::env::var("SERVO_PLUGIN_ENABLED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Self { enabled }
    }
}

impl Named for ServoNngBidFeedPlugin {
    fn name(&self) -> &'static str {
        "servo-nng"
    }
}

impl BidFeedHook for ServoNngBidFeedPlugin {
    fn start(&self, _cancel: CancellationToken) -> eyre::Result<()> {
        if self.enabled {
            tracing::info!(target: "plugins", name = "servo-nng", "SERVO plugin enabled (NNG wiring handled in config)");
        } else {
            tracing::info!(target: "plugins", name = "servo-nng", "SERVO plugin disabled");
        }
        Ok(())
    }
}
