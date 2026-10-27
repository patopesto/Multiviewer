mod common;
mod source;
mod output;
mod output_convert;
mod stats;

mod test;
mod decklink;
mod ndi;
mod omt;
mod vnc;
mod rdp;
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

pub use common::{Protocol, Frame, CpuFrame, GpuFrame};
pub use common::{PixelFormat, ConvMode, ConvUniform, TextureLayout};
pub use common::{FramePool, Framebuffer};
pub use common::{ColorRange, ColorSpace};
pub use common::{PUBLISH_INTERVAL, PollGate, publish_due, sleep_while_running};
pub use source::{
    SourceKey, SourceRef, SourceKind, SourceRuntimeConfig, SourceConfig,
    SourceRegistry, VideoSource,
};
pub use output::{OutputConfig, OutputRegistry, TextureTarget};
pub use output_convert::YuvConverter;
pub use stats::{OutputStats, SourceStats};

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
    DecklinkDiscovery, DecklinkSourceConfig, DecklinkOutputConfig, DecklinkOutputMode,
    DecklinkOutputPort, decklink_version, DecklinkVideoConnection, DecklinkVideoConnections,
    DecklinkMode,
};
pub use vnc::VncSourceConfig;
pub use rdp::{RdpColorDepth, RdpSourceConfig};

#[cfg(target_os = "macos")]
pub use syphon::{
    SyphonDiscovery, SyphonSourceConfig, SyphonOutputConfig, syphon_version,
    SyphonServerInfo, format_syphon_label, find_by_display_name as find_syphon_server,
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
