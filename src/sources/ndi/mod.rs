mod discovery;
mod output;
mod source;

use grafton_ndi::NDI;

pub use discovery::Discovery as NdiDiscovery;
pub use source::{NdiSource, NdiSourceConfig};
pub use output::{NdiOutput, NdiOutputConfig};

// external crate re-exports
pub use grafton_ndi::{
    ReceiverBandwidth as NdiReceiverBandwidth,
    ReceiverColorFormat as NdiReceiverColorFormat,
    Source as NdiSourceInfo,
};

/// Version string of the NDI runtime the app is linked against.
pub fn ndi_version() -> Option<String> {
    return NDI::version().ok();
}
