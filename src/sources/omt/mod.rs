mod discovery;
mod output;
mod source;

use openmediatransport::protocol::frame::PROTOCOL_VERSION;

pub use discovery::Discovery as OmtDiscovery;
pub use output::{OmtOutput, OmtOutputConfig};
pub use source::{OmtQuality, OmtSource, OmtSourceConfig};

// external crate re-exports
pub use openmediatransport::OmtAddress as OmtSourceInfo;

/// Version of the OMT wire protocol this build speaks.
pub fn omt_version() -> Option<String> {
    return Some(PROTOCOL_VERSION.to_string());
}
