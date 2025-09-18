use serde::Deserialize;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PluginsConfig {
    // If [servo] section is missing, SERVO is considered disabled
    pub servo: Option<ServoPluginConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServoPluginConfig {
    // Required core parameters (all-or-nothing)
    pub ws_url: String,
    pub http_url: String,
    pub el_rpc_url: String,
    pub bearer_token: String,
    pub servo_secret_key: String,
    // Optional backoff/timeouts
    pub max_reconnect_attempts: Option<u32>,
    pub reconnect_delay_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
}

static RUNTIME_PLUGINS: OnceLock<PluginsConfig> = OnceLock::new();

pub fn init_plugins_from_main_toml(path: &Path) {
    if let Ok(data) = fs::read_to_string(path) {
        if let Ok(val) = data.parse::<toml::Value>() {
            if let Some(servo_tbl) = val.get("servo").cloned() {
                if let Ok(servo_cfg) = servo_tbl.try_into::<ServoPluginConfig>() {
                    let _ = RUNTIME_PLUGINS.set(PluginsConfig {
                        servo: Some(servo_cfg),
                    });
                }
            }
        }
    }
}

pub fn load_default_plugins_config() -> Option<PluginsConfig> {
    // Use runtime-initialized config parsed from the main TOML, else None
    RUNTIME_PLUGINS.get().cloned()
}
