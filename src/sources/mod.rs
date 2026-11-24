mod common;
mod common_text;
mod output;
mod output_convert;
mod source;
mod stats;

#[cfg(target_os = "macos")]
mod avfoundation;
mod decklink;
#[cfg(target_os = "windows")]
mod directshow;
#[cfg(target_os = "windows")]
mod mediafoundation;
mod ndi;
mod omt;
mod rdp;
#[cfg(target_os = "macos")]
mod screencapturekit;
#[cfg(target_os = "windows")]
mod spout;
#[cfg(target_os = "macos")]
mod syphon;
mod test;
mod text;
mod timecode;
mod vnc;
#[cfg(target_os = "windows")]
mod windowscapture;

pub use common::{Protocol, Frame, CpuFrame, GpuFrame};
pub use common::{PixelFormat, ConvMode, ConvUniform, TextureLayout, TextureTarget};
pub use common::{RENDER_TARGET_USAGE, UPLOAD_TARGET_USAGE};
pub use common::{FramePool, Framebuffer};
pub use common::{ColorRange, ColorSpace};
pub use common::{PUBLISH_INTERVAL, PollGate, publish_due, sleep_while_running};
pub use common_text::{rasterize_text, TextAlign, TextStyle};
pub use source::{SourceKey, SourceRef, SourceKind, SourceRuntimeConfig, SourceConfig, SourceRegistry, VideoSource};
pub use output::{OutputConfig, OutputRegistry};
pub use output_convert::YuvConverter;
pub use stats::{OutputStats, SourceStats};

pub use test::{TestSourceConfig, TestPattern, RadarDirection};
pub use text::TextSourceConfig;
pub use timecode::{TimecodeInputs, TimecodeProtocol, TimecodeSourceConfig, TimecodeState, TimecodeStatus, TimecodeDisplay, ltc_devices, mtc_ports};
pub use ndi::{NdiDiscovery, NdiSourceConfig, NdiOutputConfig, ndi_version, NdiReceiverBandwidth, NdiReceiverColorFormat, NdiSourceInfo};
pub use omt::{OmtDiscovery, OmtSourceConfig, OmtOutputConfig, omt_version, OmtQuality, OmtSourceInfo};
pub use decklink::{
    DecklinkDiscovery, DecklinkSourceConfig, DecklinkOutputConfig, DecklinkOutputMode, DecklinkOutputPort, decklink_version, DecklinkVideoConnection,
    DecklinkVideoConnections, DecklinkMode,
};
pub use vnc::VncSourceConfig;
pub use rdp::{RdpColorDepth, RdpSourceConfig};

#[cfg(target_os = "macos")]
pub use syphon::{
    SyphonDiscovery, SyphonSourceConfig, SyphonOutputConfig, syphon_version, SyphonServerInfo, format_syphon_label,
    find_by_display_name as find_syphon_server,
};
#[cfg(target_os = "macos")]
pub use avfoundation::{AvFoundationDiscovery, AvFoundationSourceConfig, AvFoundationMode};
#[cfg(target_os = "macos")]
pub use screencapturekit::{ScreenCaptureKitDiscovery, ScreenCaptureKitSourceConfig, ScreenCaptureTarget, ensure_screen_capture_access_requested};

#[cfg(target_os = "windows")]
pub use spout::{SpoutDiscovery, SpoutSourceConfig, SpoutOutputConfig, spout_version};
#[cfg(target_os = "windows")]
pub use mediafoundation::{MediaFoundationDiscovery, MediaFoundationSourceConfig, MediaFoundationMode};
#[cfg(target_os = "windows")]
pub use directshow::{DirectShowDiscovery, DirectShowSourceConfig, DirectShowMode};
#[cfg(target_os = "windows")]
pub use windowscapture::{
    WindowsCaptureDiscovery, WindowsCaptureSourceConfig, WindowsCaptureTarget, WindowsCaptureBorder, WindowsCaptureCursor,
    WindowsCaptureSecondaryWindows,
};
