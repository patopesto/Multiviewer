mod discovery;
mod output;
mod source;

pub use discovery::Discovery as NdiDiscovery;
pub use source::{NdiSource, NdiSourceConfig};
pub use output::{NdiOutput, NdiOutputConfig};

// external crate re-exports
pub use grafton_ndi::{
    ReceiverBandwidth as NdiReceiverBandwidth,
    ReceiverColorFormat as NdiReceiverColorFormat,
    Source as NdiSourceInfo,
};
