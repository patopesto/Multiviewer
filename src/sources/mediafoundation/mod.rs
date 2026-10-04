mod discovery;
mod source;

pub use discovery::Discovery as MediaFoundationDiscovery;
pub use source::{MediaFoundationSource, MediaFoundationSourceConfig};
pub use source::CaptureMode as MediaFoundationMode;