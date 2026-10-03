mod discovery;
mod source;

pub use discovery::Discovery;
pub use source::{MediaFoundationSource, MediaFoundationSourceConfig};
pub use source::CaptureMode as MediaFoundationMode;