use std::sync::Arc;
use serde::{Serialize, Deserialize};

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use crate::compositor::ConvUniform;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Protocol {
    Test,
    Ndi,
    Decklink,
    #[cfg(target_os = "macos")]
    Syphon,
    #[cfg(target_os = "macos")]
    AvFoundation,
    #[cfg(target_os = "macos")]
    ScreenCaptureKit,
    #[cfg(target_os = "windows")]
    Spout,
    #[cfg(target_os = "windows")]
    MediaFoundation,
    #[cfg(target_os = "windows")]
    DirectShow,
    #[cfg(target_os = "windows")]
    WindowsCapture,
    /// A protocol that is not available on this platform. Holds the original
    /// protocol name as serialized in the project file so it round-trips
    /// unchanged when the project is opened on a platform that supports it.
    Unknown(String),
}

impl Protocol {
    /// Serialized name of a known protocol variant, or the captured name for
    /// `Unknown`.
    pub fn name(&self) -> &str {
        match self {
            Protocol::Test => "Test",
            Protocol::Ndi => "Ndi",
            Protocol::Decklink => "Decklink",
            #[cfg(target_os = "macos")]
            Protocol::Syphon => "Syphon",
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation => "AvFoundation",
            #[cfg(target_os = "macos")]
            Protocol::ScreenCaptureKit => "ScreenCaptureKit",
            #[cfg(target_os = "windows")]
            Protocol::Spout => "Spout",
            #[cfg(target_os = "windows")]
            Protocol::MediaFoundation => "MediaFoundation",
            #[cfg(target_os = "windows")]
            Protocol::DirectShow => "DirectShow",
            #[cfg(target_os = "windows")]
            Protocol::WindowsCapture => "WindowsCapture",
            Protocol::Unknown(name) => name,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Protocol::Test => "Test".to_string(),
            Protocol::Ndi => "NDI".to_string(),
            Protocol::Decklink => "DeckLink".to_string(),
            #[cfg(target_os = "macos")]
            Protocol::Syphon => "Syphon".to_string(),
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation => "AVFoundation".to_string(),
            #[cfg(target_os = "macos")]
            Protocol::ScreenCaptureKit => "macOS Screen Capture".to_string(),
            #[cfg(target_os = "windows")]
            Protocol::Spout => "Spout".to_string(),
            #[cfg(target_os = "windows")]
            Protocol::MediaFoundation => "MediaFoundation".to_string(),
            #[cfg(target_os = "windows")]
            Protocol::DirectShow => "DirectShow (WDM)".to_string(),
            #[cfg(target_os = "windows")]
            Protocol::WindowsCapture => "Windows Screen Capture".to_string(),
            Protocol::Unknown(name) => format!("{name} (Unavailable)"),
        }
    }
}

impl Serialize for Protocol {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for Protocol {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "Test" => Protocol::Test,
            "Ndi" => Protocol::Ndi,
            "Decklink" => Protocol::Decklink,
            #[cfg(target_os = "macos")]
            "Syphon" => Protocol::Syphon,
            #[cfg(target_os = "macos")]
            "AvFoundation" => Protocol::AvFoundation,
            #[cfg(target_os = "macos")]
            "ScreenCaptureKit" => Protocol::ScreenCaptureKit,
            #[cfg(target_os = "windows")]
            "Spout" => Protocol::Spout,
            #[cfg(target_os = "windows")]
            "MediaFoundation" => Protocol::MediaFoundation,
            #[cfg(target_os = "windows")]
            "DirectShow" => Protocol::DirectShow,
            #[cfg(target_os = "windows")]
            "WindowsCapture" => Protocol::WindowsCapture,
            other => Protocol::Unknown(other.to_string()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    // RGBA 4:4:4, 8 bit per channel, 4 bytes per pixel.
    Rgba8,
    // BGRA 4:4:4, 8 bit per channel, 4 bytes per pixel.
    Bgra8,
    // YUV 4:2:2 (UYVY), Packed into 2 bytes per pixel.
    Uyvy422,
    // YUV 4:2:2 (YUY2), Packed into 2 bytes per pixel; chroma order swapped vs UYVY (Y0 U Y1 V instead of U Y0 V Y1).
    Yuy2,
    // YUV 4:2:0 (NV12): full-res Y plane followed by a half-res interleaved U/V plane.
    Nv12,
}

impl PixelFormat {
    pub fn label(&self) -> &'static str {
        match self {
            PixelFormat::Rgba8 => "RGBA8",
            PixelFormat::Bgra8 => "BGRA8",
            PixelFormat::Uyvy422 => "UYVY",
            PixelFormat::Yuy2 => "YUY2",
            PixelFormat::Nv12 => "NV12",
        }
    }
}

#[derive(Clone)]
pub struct CpuFrame {
    pub data: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
    pub fmt: PixelFormat,
    /// Row stride in bytes; 0 means tightly packed (`w * bpp`).
    pub pitch: u32,
    /// Monotonic per-source counter; compositor uploads only when this changes.
    pub seq: u64,
}

#[allow(dead_code)] // No GPU-native source on Linux yet.
#[derive(Clone)]
pub struct GpuFrame {
    pub bg: Arc<wgpu::BindGroup>,
    pub w: u32,
    pub h: u32,
    pub seq: u64,
    pub flip_v: bool, // for syphon which has it's UV bottom-up.
}

#[derive(Clone)]
pub enum Frame {
    Cpu(CpuFrame),
    #[allow(dead_code)] // No GPU-native source on Linux yet.
    Gpu(GpuFrame),
}

impl Frame {
    /// The source's monotonic frame counter, for consumption accounting.
    pub fn seq(&self) -> u64 {
        return match self {
            Frame::Cpu(f) => f.seq,
            Frame::Gpu(f) => f.seq,
        };
    }
}