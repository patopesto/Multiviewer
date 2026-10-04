mod discovery;
mod output;
mod source;

pub use discovery::Discovery as SyphonDiscovery;
pub use discovery::format_syphon_label;
pub use source::{SyphonSource, SyphonSourceConfig};
pub use output::{SyphonOutput, SyphonOutputConfig};

// external crate re-exports
pub use syphon_core::ServerInfo as SyphonServerInfo;