mod discovery;
mod source;

pub use discovery::Discovery as OmtDiscovery;
pub use source::{OmtQuality, OmtSource, OmtSourceConfig};

// external crate re-exports
pub use openmediatransport::OmtAddress as OmtSourceInfo;
