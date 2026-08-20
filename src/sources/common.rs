use std::sync::Arc;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Protocol {
    Test,
    Ndi,
    Syphon,
    Decklink,
}

impl Protocol {
    pub fn label(&self) -> &'static str {
        match self {
            Protocol::Test => "Test",
            Protocol::Ndi => "NDI",
            Protocol::Syphon => "Syphon",
            Protocol::Decklink => "DeckLink",
        }
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
    pub seq: u64,
}

#[derive(Clone)]
pub enum Frame {
    Cpu(CpuFrame),
    Syphon(SyphonFrame),
}