pub mod decklink;
pub mod output;
pub mod ndi;
pub mod source;
pub mod test;

#[cfg(target_os = "macos")]
pub mod syphon;

pub use source::{
    ConvUniform, CpuFrame, Frame, PixelFormat, RestartResult, SourceId, SourceKind, SourceRegistry,
    SourceStats, SyphonFrame, VideoSource,
};
pub use output::{
    VideoOutput, OutputRegistry,
};
pub use decklink::source::{DecklinkConfig, DecklinkSource};
pub use decklink::VideoConnections;
pub use ndi::source::{NdiConfig, NdiSource};
pub use test::{TestConfig, TestSource};

#[cfg(target_os = "macos")]
pub use syphon::source::{SyphonConfig, SyphonSource};
