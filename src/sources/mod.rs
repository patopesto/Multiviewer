pub mod decklink;
pub mod output;
pub mod ndi;
pub mod source;
pub mod test;

#[cfg(target_os = "macos")]
pub mod syphon;

#[allow(unused_imports)]
pub use source::{
    ConvUniform, CpuFrame, Frame, PixelFormat, RestartResult, SourceId, SourceKind, SourceRegistry,
    SourceStats, SyphonFrame, VideoSource,
};
#[allow(unused_imports)]
pub use output::{
    OutputId, OutputKind, OutputRegistry, OutputStats,
};

#[allow(unused_imports)]
pub use decklink::source::{DecklinkConfig, DecklinkSource};
#[allow(unused_imports)]
pub use decklink::VideoConnections;
#[allow(unused_imports)]
pub use ndi::{NdiSource, NdiSourceConfig, NdiOutput, NdiOutputConfig};
#[allow(unused_imports)]
pub use test::{TestConfig, TestSource};

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use syphon::{SyphonSource, SyphonSourceConfig, SyphonOutput, SyphonOutputConfig};
