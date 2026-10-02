mod discovery;
mod output;
mod source;

pub use multiviewer_decklink::{VideoConnection, VideoConnections, DisplayMode};
pub use discovery::Discovery;
pub use source::{DecklinkSource, DecklinkSourceConfig};
pub use output::{DecklinkOutput, DecklinkOutputConfig};
