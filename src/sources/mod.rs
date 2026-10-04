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
pub use ndi::{NdiSourceConfig, NdiOutputConfig};
pub use ndi::Discovery as NdiDiscovery;
pub use decklink::{DecklinkSourceConfig, DecklinkOutputConfig, VideoConnection, VideoConnections, DisplayMode};
pub use decklink::Discovery as DecklinkDiscovery;

#[cfg(target_os = "macos")]
pub use syphon::{SyphonSourceConfig, SyphonOutputConfig, format_syphon_label};
#[cfg(target_os = "macos")]
pub use syphon::Discovery as SyphonDiscovery;
#[cfg(target_os = "macos")]
pub use avfoundation::AvFoundationSourceConfig;
#[cfg(target_os = "macos")]
pub use avfoundation::Discovery as AvFoundationDiscovery;
#[cfg(target_os = "macos")]
pub use screencapturekit::{ScreenCaptureKitSourceConfig, ensure_screen_capture_access_requested};
#[cfg(target_os = "macos")]
pub use screencapturekit::Discovery as ScreenCaptureKitDiscovery;

#[cfg(target_os = "windows")]
pub use spout::{SpoutSourceConfig, SpoutOutputConfig};
#[cfg(target_os = "windows")]
pub use spout::Discovery as SpoutDiscovery;
#[cfg(target_os = "windows")]
pub use mediafoundation::{MediaFoundationSourceConfig, MediaFoundationMode};
#[cfg(target_os = "windows")]
pub use mediafoundation::Discovery as MediaFoundationDiscovery;
#[cfg(target_os = "windows")]
pub use directshow::{DirectShowSourceConfig, DirectShowMode};
#[cfg(target_os = "windows")]
pub use directshow::Discovery as DirectShowDiscovery;
#[cfg(target_os = "windows")]
pub use windowscapture::{WindowsCaptureSourceConfig, WindowsCaptureBorder, WindowsCaptureCursor, WindowsCaptureSecondaryWindows};
#[cfg(target_os = "windows")]
pub use windowscapture::Discovery as WindowsCaptureDiscovery;