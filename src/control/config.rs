use serde::{Deserialize, Serialize};

use super::http::HttpControlConfig;
use super::osc::OscControlConfig;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ControlConfig {
    #[serde(default)]
    pub osc: OscControlConfig,
    #[serde(default)]
    pub http: HttpControlConfig,
}
