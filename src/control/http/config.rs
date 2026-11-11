use serde::{Deserialize, Serialize};

/// HTTP API and web UI listener. The web client is served from the same port.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HttpControlConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
}

impl Default for HttpControlConfig {
    fn default() -> Self {
        return Self {
            enabled: false,
            bind: "127.0.0.1".to_string(),
            port: 8090,
        };
    }
}
