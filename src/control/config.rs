use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use super::osc::OscControlConfig;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ControlConfig {
    #[serde(default)]
    pub osc: OscControlConfig,
}

/// Whether `bind` restricts the listener to this machine.
pub fn is_loopback_bind(bind: &str) -> bool {
    if bind == "localhost" {
        return true;
    }
    return bind
        .parse::<IpAddr>()
        .map(|address| address.is_loopback())
        .unwrap_or(false);
}
