mod discovery;
mod output;
mod source;

pub use discovery::Discovery as DecklinkDiscovery;
pub use source::{DecklinkSource, DecklinkSourceConfig};
pub use output::{DecklinkOutput, DecklinkOutputConfig};

// external crate re-exports
pub use multiviewer_decklink::{
    VideoConnection as DecklinkVideoConnection,
    VideoConnections as DecklinkVideoConnections,
    DisplayMode as DecklinkMode,
};
