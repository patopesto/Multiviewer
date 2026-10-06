mod discovery;
mod output;
mod source;

pub use discovery::Discovery as SpoutDiscovery;
pub use source::{SpoutSource, SpoutSourceConfig};
pub use output::{SpoutOutput, SpoutOutputConfig};

/// Version string of the Spout SDK the app is linked against.
pub fn spout_version() -> Option<String> {
    return Some(spout2::sdk_version());
}
