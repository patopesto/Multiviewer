pub mod discovery;
pub mod source;

pub use multiviewer_decklink::{VideoConnection, VideoConnections};
pub use discovery::Discovery;
#[allow(unused_imports)]
pub use source::{DecklinkSource, DecklinkConfig};
