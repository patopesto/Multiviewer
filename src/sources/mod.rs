pub mod common;
pub mod source;
pub mod output;

pub mod test;
pub mod decklink;
pub mod ndi;
#[cfg(target_os = "macos")]
pub mod syphon;

#[allow(unused_imports)]
pub use common::{Protocol, PixelFormat, Frame, CpuFrame, SyphonFrame};

#[allow(unused_imports)]
pub use source::{
    SourceId, SourceKind, VideoSource, SourceConfig, SourceStats,
    SourceRegistry, RestartResult, ConvUniform,
};
#[allow(unused_imports)]
pub use output::{
    OutputId, OutputKind, VideoOutput, OutputConfig, OutputStats,
    OutputRegistry,
};

#[allow(unused_imports)]
pub use decklink::source::{DecklinkSource, DecklinkSourceConfig};
#[allow(unused_imports)]
pub use decklink::output::{DecklinkOutput, DecklinkOutputConfig};
#[allow(unused_imports)]
pub use decklink::VideoConnections;
#[allow(unused_imports)]
pub use ndi::{NdiSource, NdiSourceConfig, NdiOutput, NdiOutputConfig};
#[allow(unused_imports)]
pub use test::{TestSource, TestSourceConfig};

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use syphon::{SyphonSource, SyphonSourceConfig, SyphonOutput, SyphonOutputConfig};
