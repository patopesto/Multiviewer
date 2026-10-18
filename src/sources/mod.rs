mod common;
mod source;
mod output;
mod stats;
mod framebuffer;

mod test;
mod decklink;
mod ndi;
mod omt;
mod vnc;
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

pub use common::{Protocol, PixelFormat, Frame, CpuFrame};
pub use common::{GpuFrame, ConvUniform};
pub use source::{
    SourceKey, SourceRef, SourceKind, SourceRuntimeConfig, SourceConfig,
    SourceRegistry, VideoSource, FramePool,
};
pub use output::{OutputConfig, OutputRegistry};
pub use stats::{OutputStats, SourceStats};
pub use framebuffer::Framebuffer;

pub use test::{TestSourceConfig, TestPattern, RadarDirection};
pub use ndi::{
    NdiDiscovery, NdiSourceConfig, NdiOutputConfig, ndi_version,
    NdiReceiverBandwidth, NdiReceiverColorFormat, NdiSourceInfo,
};
pub use omt::{
    OmtDiscovery, OmtSourceConfig, OmtOutputConfig, omt_version,
    OmtQuality, OmtSourceInfo, 
};
pub use decklink::{
    DecklinkDiscovery, DecklinkSourceConfig, DecklinkOutputConfig, decklink_version,
    DecklinkVideoConnection, DecklinkVideoConnections, DecklinkMode,
};
pub use vnc::VncSourceConfig;

#[cfg(target_os = "macos")]
pub use syphon::{
    SyphonDiscovery, SyphonSourceConfig, SyphonOutputConfig, syphon_version,
    SyphonServerInfo, format_syphon_label,
};
#[cfg(target_os = "macos")]
pub use avfoundation::{AvFoundationDiscovery, AvFoundationSourceConfig, AvFoundationMode};
#[cfg(target_os = "macos")]
pub use screencapturekit::{ScreenCaptureKitDiscovery, ScreenCaptureKitSourceConfig, ensure_screen_capture_access_requested};

#[cfg(target_os = "windows")]
pub use spout::{SpoutDiscovery, SpoutSourceConfig, SpoutOutputConfig, spout_version};
#[cfg(target_os = "windows")]
pub use mediafoundation::{MediaFoundationDiscovery, MediaFoundationSourceConfig, MediaFoundationMode};
#[cfg(target_os = "windows")]
pub use directshow::{DirectShowDiscovery, DirectShowSourceConfig, DirectShowMode};
#[cfg(target_os = "windows")]
pub use windowscapture::{
    WindowsCaptureDiscovery, WindowsCaptureSourceConfig,
    WindowsCaptureBorder, WindowsCaptureCursor, WindowsCaptureSecondaryWindows,
};
