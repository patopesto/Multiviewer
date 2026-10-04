mod common;
mod source;
mod output;

mod test;
mod decklink;
mod ndi;
#[cfg(target_os = "macos")]
mod syphon;
#[cfg(target_os = "macos")]
mod avfoundation;
#[cfg(target_os = "macos")]
mod screencapturekit;
#[cfg(target_os = "windows")]
mod spout;
#[cfg(target_os = "windows")]
mod mediafoundation;
#[cfg(target_os = "windows")]
mod directshow;
#[cfg(target_os = "windows")]
mod windowscapture;

pub use common::{Protocol, PixelFormat, Frame, CpuFrame, GpuFrame};
pub use source::{
    SourceKey, SourceRef, SourceKind, SourceRuntimeConfig, SourceConfig,
    SourceStats, SourceRegistry, VideoSource, ConvUniform, FramePool,
};
pub use output::{OutputConfig, OutputRegistry};

pub use test::{TestSourceConfig, TestPattern, RadarDirection};
pub use ndi::{NdiDiscovery, NdiSourceConfig, NdiOutputConfig, NdiReceiverBandwidth, NdiReceiverColorFormat, NdiSourceInfo};
pub use decklink::{
    DecklinkDiscovery, DecklinkSourceConfig, DecklinkOutputConfig,
    DecklinkVideoConnection, DecklinkVideoConnections, DecklinkMode,
};

#[cfg(target_os = "macos")]
pub use syphon::{SyphonDiscovery, SyphonSourceConfig, SyphonOutputConfig, format_syphon_label, SyphonServerInfo};
#[cfg(target_os = "macos")]
pub use avfoundation::{AvFoundationDiscovery, AvFoundationSourceConfig};
#[cfg(target_os = "macos")]
pub use screencapturekit::{ScreenCaptureKitDiscovery, ScreenCaptureKitSourceConfig, ensure_screen_capture_access_requested};

#[cfg(target_os = "windows")]
pub use spout::{SpoutDiscovery, SpoutSourceConfig, SpoutOutputConfig};
#[cfg(target_os = "windows")]
pub use mediafoundation::{MediaFoundationDiscovery, MediaFoundationSourceConfig, MediaFoundationMode};
#[cfg(target_os = "windows")]
pub use directshow::{DirectShowDiscovery, DirectShowSourceConfig, DirectShowMode};
#[cfg(target_os = "windows")]
pub use windowscapture::{
    WindowsCaptureDiscovery, WindowsCaptureSourceConfig,
    WindowsCaptureBorder, WindowsCaptureCursor, WindowsCaptureSecondaryWindows,
};
