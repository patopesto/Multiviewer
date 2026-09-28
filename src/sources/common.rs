use std::sync::Arc;
use serde::{Serialize, Deserialize};

#[derive(Clone, PartialEq, Debug)]
pub enum Protocol {
    Test,
    Ndi,
    #[cfg(target_os = "macos")]
    Syphon,
    Decklink,
    #[cfg(target_os = "macos")]
    AvFoundation,
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
            #[cfg(target_os = "macos")]
            Protocol::Syphon => "Syphon",
            Protocol::Decklink => "Decklink",
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation => "AvFoundation",
            Protocol::Unknown(name) => name,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Protocol::Test => "Test".to_string(),
            Protocol::Ndi => "NDI".to_string(),
            #[cfg(target_os = "macos")]
            Protocol::Syphon => "Syphon".to_string(),
            Protocol::Decklink => "DeckLink".to_string(),
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation => "AVFoundation".to_string(),
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
            #[cfg(target_os = "macos")]
            "Syphon" => Protocol::Syphon,
            "Decklink" => Protocol::Decklink,
            #[cfg(target_os = "macos")]
            "AvFoundation" => Protocol::AvFoundation,
            other => Protocol::Unknown(other.to_string()),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    // RGBA 4:4:4, 8 bit per channel, 4 bytes per pixel.
    Rgba8,
    // BGRA 4:4:4, 8 bit per channel, 4 bytes per pixel.
    Bgra8,
    // YUV 4:2:2 (UYVY), Packed into 2 bytes per pixel.
    Uyvy422,
}

impl PixelFormat {
    pub fn label(&self) -> &'static str {
        match self {
            PixelFormat::Rgba8 => "RGBA8",
            PixelFormat::Bgra8 => "BGRA8",
            PixelFormat::Uyvy422 => "UYVY",
        }
    }
}

#[derive(Clone)]
pub struct CpuFrame {
    pub data: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
    pub fmt: PixelFormat,
    /// Monotonic per-source counter; compositor uploads only when this changes.
    pub seq: u64,
}

#[derive(Clone)]
pub struct SyphonFrame {
    pub bg: Arc<wgpu::BindGroup>,
    pub w: u32,
    pub h: u32,
    #[allow(dead_code)]
    pub seq: u64,
}

#[derive(Clone)]
pub enum Frame {
    Cpu(CpuFrame),
    Syphon(SyphonFrame),
}