use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Protocol {
    Test,
    Ndi,
    Omt,
    Decklink,
    Vnc,
    Rdp,
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
            Protocol::Omt => "Omt",
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
            Protocol::Vnc => "Vnc",
            Protocol::Rdp => "Rdp",
            Protocol::Unknown(name) => name,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Protocol::Test => "Test".to_string(),
            Protocol::Ndi => "NDI".to_string(),
            Protocol::Omt => "OMT".to_string(),
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
            Protocol::Vnc => "VNC".to_string(),
            Protocol::Rdp => "RDP".to_string(),
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
            "Omt" => Protocol::Omt,
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
            "Vnc" => Protocol::Vnc,
            "Rdp" => Protocol::Rdp,
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

    pub const fn bpp(self) -> u32 {
        return match self {
            PixelFormat::Rgba8 | PixelFormat::Bgra8 => 4,
            PixelFormat::Uyvy422 | PixelFormat::Yuy2 => 2,
            PixelFormat::Nv12 => 1,
        };
    }

    pub fn conv_mode(self, color: ColorSpace) -> ConvMode {
        return match self {
            PixelFormat::Rgba8 | PixelFormat::Bgra8 => ConvMode::Passthrough,
            PixelFormat::Uyvy422 => {
                if color.is_bt709() { ConvMode::UyvyBt709 } else { ConvMode::UyvyBt601 }
            }
            PixelFormat::Yuy2 => {
                if color.is_bt709() { ConvMode::Yuy2Bt709 } else { ConvMode::Yuy2Bt601 }
            }
            PixelFormat::Nv12 => {
                if color.is_bt709() { ConvMode::Nv12Bt709 } else { ConvMode::Nv12Bt601 }
            }
        };
    }

    pub fn texture_layout(self, width: u32, height: u32) -> TextureLayout {
        let bpp = self.bpp();
        let color = colorspace_for_height(height);
        let (texture_format, tex_w, tex_h) = match self {
            PixelFormat::Rgba8 => (wgpu::TextureFormat::Rgba8Unorm, width, height),
            PixelFormat::Bgra8 => (wgpu::TextureFormat::Bgra8Unorm, width, height),
            PixelFormat::Uyvy422 | PixelFormat::Yuy2 => {
                (wgpu::TextureFormat::Rgba8Unorm, width / 2, height)
            }
            PixelFormat::Nv12 => {
                (wgpu::TextureFormat::R8Unorm, width, height + height / 2)
            }
        };
        return TextureLayout {
            width,
            height,
            texture_format,
            tex_w,
            tex_h,
            bpp,
            conv_mode: self.conv_mode(color),
        };
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    Bt601 = 0,
    Bt709 = 1,
}

impl ColorSpace {
    pub fn is_bt709(self) -> bool {
        return self == Self::Bt709;
    }

    pub const fn code(self) -> u32 {
        return self as u32;
    }

    /// Output colorimetry for SDI/NDI raster height.
    pub fn for_output_height(height: u32) -> Self {
        return colorspace_for_height(height);
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorRange {
    #[default]
    Limited = 0,
    // Full range is available for protocols that negotiate it.
    #[allow(dead_code)]
    Full = 1,
}

impl ColorRange {
    pub const fn code(self) -> u32 {
        return self as u32;
    }
}

pub fn colorspace_for_height(height: u32) -> ColorSpace {
    return if height > 576 { ColorSpace::Bt709 } else { ColorSpace::Bt601 };
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvMode {
    Passthrough = 0,
    UyvyBt601 = 1,
    UyvyBt709 = 2,
    Yuy2Bt601 = 3,
    Yuy2Bt709 = 4,
    Nv12Bt601 = 5,
    Nv12Bt709 = 6,
}

impl From<ConvMode> for u32 {
    fn from(mode: ConvMode) -> Self {
        return mode as u32;
    }
}

/// Uniform block consumed by the compositor's fragment shader.
/// Must stay in sync with the `ConvUniform` struct inside `SHADER`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ConvUniform {
    pub mode: u32, // ConvMode
    pub width: f32,
    pub height: f32,
    pub _pad: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureLayout {
    pub width: u32,
    pub height: u32,
    pub texture_format: wgpu::TextureFormat,
    /// 4:2:2 packs two pixels per texel, so this is half the frame width.
    pub tex_w: u32,
    /// NV12 stores its interleaved UV plane after Y, so this is 1.5x the frame height.
    pub tex_h: u32,
    pub bpp: u32,
    pub conv_mode: ConvMode,
}

impl TextureLayout {
    pub fn row_bytes(&self) -> u32 {
        return self.width * self.bpp;
    }

    pub fn buffer_bytes(&self) -> u64 {
        return u64::from(self.row_bytes()) * u64::from(self.tex_h);
    }
}

/// GPU texture paired with its view, layout and creation usage.
pub struct TextureTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub layout: TextureLayout,
    pub usage: wgpu::TextureUsages,
}

/// Render target that is also readable and copyable to a readback buffer.
pub const RENDER_TARGET_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC);

/// Upload target sampled by a bind group.
pub const UPLOAD_TARGET_USAGE: wgpu::TextureUsages =
    wgpu::TextureUsages::TEXTURE_BINDING.union(wgpu::TextureUsages::COPY_DST);

impl TextureTarget {
    /// Recreate the target when its layout or usage changes.
    pub fn ensure(
        slot: &mut Option<Self>,
        device: &wgpu::Device,
        layout: TextureLayout,
        usage: wgpu::TextureUsages,
        label: &str,
    ) {
        if slot
            .as_ref()
            .is_some_and(|target| target.layout == layout && target.usage == usage)
        {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: layout.tex_w,
                height: layout.tex_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: layout.texture_format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        *slot = Some(Self {
            texture,
            view,
            layout,
            usage,
        });
    }

    /// Wrap an externally-created texture, such as a CV IOSurface.
    pub fn from_parts(
        texture: wgpu::Texture,
        view: wgpu::TextureView,
        layout: TextureLayout,
        usage: wgpu::TextureUsages,
    ) -> Self {
        return Self {
            texture,
            view,
            layout,
            usage,
        };
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

/// Reusable full-frame buffers to be used by VideoSource producers
#[derive(Default)]
pub struct FramePool {
    free: Vec<Vec<u8>>,
    pending: Option<Arc<Vec<u8>>>,
}

impl FramePool {
    pub fn new() -> Self {
        return Self::default();
    }

    /// A buffer of exactly `len` bytes, reusing the deferred one
    pub fn take(&mut self, len: usize) -> Vec<u8> {
        if let Some(arc) = self.pending.take() {
            match Arc::try_unwrap(arc) {
                Ok(vec) => self.free.push(vec),
                Err(arc) => self.pending = Some(arc),
            }
        }
        if let Some(i) = self.free.iter().position(|v| v.capacity() >= len) {
            let mut vec = self.free.swap_remove(i);
            vec.resize(len, 0);
            return vec;
        }
        return vec![0u8; len];
    }

    /// Hand back a buffer that was taken but never published.
    pub fn release(&mut self, vec: Vec<u8>) {
        self.free.push(vec);
    }

    /// Recycle the frame just published into the slot.
    pub fn give(&mut self, frame: Option<Frame>) {
        let Some(Frame::Cpu(cpu)) = frame else {
            return;
        };
        match Arc::try_unwrap(cpu.data) {
            Ok(vec) => self.free.push(vec),
            // Compositor still holds the clone: retry at the next `take`.
            Err(arc) => {
                if self.pending.is_none() {
                    self.pending = Some(arc);
                }
            }
        }
    }
}

/// A software framebuffer that protocol producers blit incremental updates
/// into. VNC feeds RFB rectangles; RDP will feed desktop updates once its
/// client exposes them.
pub struct Framebuffer {
    data: Vec<u8>,
    width: u32,
    height: u32,
    format: PixelFormat, // only BGRA for now
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let format = PixelFormat::Bgra8;
        return Self {
            data: vec![0; buffer_size(width, height, format.bpp() as usize)],
            width,
            height,
            format,
        };
    }

    pub fn width(&self) -> u32 {
        return self.width;
    }

    pub fn height(&self) -> u32 {
        return self.height;
    }

    pub fn format(&self) -> PixelFormat {
        return self.format;
    }

    fn bpp(&self) -> usize {
        return self.format.bpp() as usize;
    }

    /// Reallocate for a new resolution, keeping the pixels that still fit. A size
    /// change must not wipe a desktop already painted: the server has to repaint
    /// the whole screen after a real resize, so the stale overlap is overwritten
    /// anyway, while a spurious size announcement would otherwise blank the tile.
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }
        let bpp = self.bpp();
        let mut data = vec![0; buffer_size(width, height, bpp)];
        let copy_bytes = self.width.min(width) as usize * bpp;
        let src_stride = self.width as usize * bpp;
        let dst_stride = width as usize * bpp;
        for row in 0..self.height.min(height) as usize {
            let src = row * src_stride;
            let dst = row * dst_stride;
            data[dst..dst + copy_bytes].copy_from_slice(&self.data[src..src + copy_bytes]);
        }
        self.data = data;
        self.width = width;
        self.height = height;
    }

    /// Number of bytes a snapshot of this framebuffer occupies.
    pub fn byte_len(&self) -> usize {
        return self.data.len();
    }

    /// Copy a tightly packed `width`x`height` pixel rectangle to (`x`, `y`),
    /// clipped to the framebuffer. `pixels` must hold `width * height * bpp`.
    pub fn blit(&mut self, x: u32, y: u32, width: u32, height: u32, pixels: &[u8]) {
        let bpp = self.bpp();
        let src_stride = width as usize * bpp;
        if src_stride * height as usize > pixels.len() {
            return;
        }
        let x_end = x.saturating_add(width).min(self.width);
        let y_end = y.saturating_add(height).min(self.height);
        if x >= x_end || y >= y_end {
            return;
        }
        let row_bytes = (x_end - x) as usize * bpp;
        let dst_stride = self.width as usize * bpp;
        for row in y..y_end {
            let src = (row - y) as usize * src_stride;
            let dst = row as usize * dst_stride + x as usize * bpp;
            self.data[dst..dst + row_bytes].copy_from_slice(&pixels[src..src + row_bytes]);
        }
    }

    /// Copy the `width`x`height` region at (`src_x`, `src_y`) to
    /// (`dst_x`, `dst_y`), the RFB CopyRect operation. Overlapping regions are
    /// handled: rows are copied in the direction that avoids clobbering a row
    /// before it is read.
    pub fn copy_rect(
        &mut self,
        dst_x: u32,
        dst_y: u32,
        src_x: u32,
        src_y: u32,
        width: u32,
        height: u32,
    ) {
        let width = width
            .min(self.width.saturating_sub(src_x))
            .min(self.width.saturating_sub(dst_x));
        let height = height
            .min(self.height.saturating_sub(src_y))
            .min(self.height.saturating_sub(dst_y));
        if width == 0 || height == 0 {
            return;
        }
        let bpp = self.bpp();
        let stride = self.width as usize * bpp;
        let row_bytes = width as usize * bpp;
        for i in 0..height {
            let row = if dst_y <= src_y { i } else { height - 1 - i };
            let src = (src_y + row) as usize * stride + src_x as usize * bpp;
            let dst = (dst_y + row) as usize * stride + dst_x as usize * bpp;
            self.data.copy_within(src..src + row_bytes, dst);
        }
    }

    /// Copy the whole framebuffer into `dst`, reusing its allocation.
    pub fn snapshot_into(&self, dst: &mut Vec<u8>) {
        dst.clone_from(&self.data);
    }
}

fn buffer_size(width: u32, height: u32, bpp: usize) -> usize {
    return width as usize * height as usize * bpp;
}

/// Granularity of the retry sleep in [`sleep_while_running`].
const RETRY_SLICE: Duration = Duration::from_millis(50);
/// Floor between two published frames; a full frame is a whole-frame copy.
pub const PUBLISH_INTERVAL: Duration = Duration::from_millis(16);
/// Never hold a dirty frame longer than this, even mid-stream.
const MAX_PUBLISH_INTERVAL: Duration = Duration::from_millis(33);

/// Sleep for `duration`, returning early once `running` clears, so a source
/// thread joins promptly instead of waiting out a retry.
pub async fn sleep_while_running(running: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while running.load(Ordering::Relaxed) && Instant::now() < deadline {
        let slice = RETRY_SLICE.min(deadline.saturating_duration_since(Instant::now()));
        tokio::time::sleep(slice).await;
    }
}

/// Whether to publish. `settle` is the quiet time that marks the end of a burst;
/// the fallback stops a source that never pauses from freezing the tile.
pub fn publish_due(settle: Duration, quiet: Duration, since_publish: Duration) -> bool {
    if since_publish < PUBLISH_INTERVAL {
        return false;
    }
    return quiet >= settle || since_publish >= MAX_PUBLISH_INTERVAL;
}

/// Idle gap between protocol discovery polls. Protocols whose discovery costs
/// vendor-framework work (DeckLink enumerates IOKit driver classes, screen
/// capture enumerates every running app) poll this slowly until the UI asks for
/// a refresh.
pub const DISCOVERY_INTERVAL: Duration = Duration::from_secs(30);
/// How often a waiting discovery thread rechecks its refresh request.
const DISCOVERY_WAKE_SLICE: Duration = Duration::from_millis(200);

/// Poll cadence for a discovery thread: wait out [`DISCOVERY_INTERVAL`], or
/// wake as soon as the UI requests a refresh.
#[derive(Clone)]
pub struct PollGate {
    refresh: Arc<AtomicBool>,
}

impl PollGate {
    pub fn new() -> Self {
        return Self {
            refresh: Arc::new(AtomicBool::new(false)),
        };
    }

    /// Ask the polling thread to poll now instead of waiting out the interval.
    pub fn request(&self) {
        self.refresh.store(true, Ordering::Release);
    }

    /// Block until the next poll is due, returning early on [`PollGate::request`].
    pub fn wait(&self) {
        let deadline = Instant::now() + DISCOVERY_INTERVAL;
        while Instant::now() < deadline {
            if self.refresh.swap(false, Ordering::AcqRel) {
                return;
            }
            thread::sleep(DISCOVERY_WAKE_SLICE);
        }
    }
}

impl Default for PollGate {
    fn default() -> Self {
        return Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorspace_switches_above_sd_height() {
        assert_eq!(colorspace_for_height(576), ColorSpace::Bt601);
        assert_eq!(colorspace_for_height(577), ColorSpace::Bt709);
        assert_eq!(ColorSpace::for_output_height(1080), ColorSpace::Bt709);
        assert_eq!(ColorSpace::for_output_height(2160), ColorSpace::Bt709);
    }

    #[test]
    fn texture_layout_row_and_buffer_bytes() {
        let uyvy = PixelFormat::Uyvy422.texture_layout(1280, 720);
        assert_eq!(uyvy.row_bytes(), 1280 * 2);
        assert_eq!(uyvy.buffer_bytes(), 1280 * 2 * 720);

        let nv12 = PixelFormat::Nv12.texture_layout(1920, 1080);
        assert_eq!(nv12.row_bytes(), 1920);
        assert_eq!(nv12.buffer_bytes(), 1920 * 1620);
    }

    /// A held Arc defers reclaim; once released the same buffer is reused
    /// instead of allocating.
    #[test]
    fn frame_pool_defers_reclaim_until_released() {
        let mut pool = FramePool::new();
        let first = pool.take(64);
        let ptr = first.as_ptr();
        let data = Arc::new(first);
        let held = data.clone();
        pool.give(Some(Frame::Cpu(CpuFrame {
            data,
            w: 16,
            h: 4,
            fmt: PixelFormat::Rgba8,
            pitch: 0,
            seq: 0,
        })));

        // The clone is still held: this one must come from a fresh allocation.
        let second = pool.take(64);
        assert_ne!(second.as_ptr(), ptr);
        drop(second);

        // Clone released: the deferred buffer comes back.
        drop(held);
        let third = pool.take(64);
        assert_eq!(third.as_ptr(), ptr);
        assert!(pool.pending.is_none());
    }

    #[test]
    fn framebuffer_reports_bgra() {
        let fb = Framebuffer::new(1, 1);
        assert_eq!(fb.format(), PixelFormat::Bgra8);
        assert_eq!(fb.format().bpp() as usize, 4);
    }

    /// A 4x4 framebuffer whose pixel value encodes its position.
    fn grid() -> Framebuffer {
        let mut fb = Framebuffer::new(4, 4);
        for y in 0..4u32 {
            for x in 0..4u32 {
                fb.blit(x, y, 1, 1, &[x as u8, y as u8, 0, 0]);
            }
        }
        return fb;
    }

    fn pixel(fb: &Framebuffer, x: u32, y: u32) -> [u8; 4] {
        let bpp = fb.format().bpp() as usize;
        let offset = (y as usize * fb.width as usize + x as usize) * bpp;
        return fb.data[offset..offset + bpp].try_into().unwrap();
    }

    #[test]
    fn blit_writes_at_the_requested_offset() {
        let mut fb = Framebuffer::new(4, 4);
        fb.blit(1, 2, 2, 1, &[9, 9, 9, 9, 8, 8, 8, 8]);
        assert_eq!(pixel(&fb, 1, 2), [9, 9, 9, 9]);
        assert_eq!(pixel(&fb, 2, 2), [8, 8, 8, 8]);
        assert_eq!(pixel(&fb, 0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn blit_clips_to_the_framebuffer() {
        let mut fb = Framebuffer::new(4, 4);
        // A 3x3 rect at (3,3) keeps only its top-left pixel.
        fb.blit(3, 3, 3, 3, &[1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3,
                             4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 6, 6,
                             7, 7, 7, 7, 8, 8, 8, 8, 9, 9, 9, 9]);
        assert_eq!(pixel(&fb, 3, 3), [1, 1, 1, 1]);
    }

    #[test]
    fn blit_ignores_short_buffers_and_out_of_bounds_rects() {
        let mut fb = Framebuffer::new(4, 4);
        fb.blit(0, 0, 2, 2, &[1, 1, 1, 1]); // too short
        assert_eq!(pixel(&fb, 0, 0), [0, 0, 0, 0]);
        fb.blit(4, 0, 1, 1, &[1, 1, 1, 1]); // entirely outside
        assert_eq!(pixel(&fb, 3, 0), [0, 0, 0, 0]);
    }

    /// Scrolling down (destination below source) must not clobber source rows
    /// before they are read.
    #[test]
    fn copy_rect_handles_overlap_both_ways() {
        let mut fb = grid();
        fb.copy_rect(0, 1, 0, 0, 4, 3);
        assert_eq!(pixel(&fb, 0, 1), [0, 0, 0, 0]);
        assert_eq!(pixel(&fb, 2, 3), [2, 2, 0, 0]);

        let mut fb = grid();
        fb.copy_rect(0, 0, 0, 1, 4, 3);
        assert_eq!(pixel(&fb, 0, 0), [0, 1, 0, 0]);
        assert_eq!(pixel(&fb, 3, 2), [3, 3, 0, 0]);
    }

    #[test]
    fn copy_rect_clips_to_the_framebuffer() {
        let mut fb = grid();
        fb.copy_rect(3, 3, 0, 0, 2, 2);
        assert_eq!(pixel(&fb, 3, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn resize_changes_the_buffer_size() {
        let mut fb = grid();
        fb.resize(2, 2);
        assert_eq!(fb.byte_len(), 2 * 2 * fb.format().bpp() as usize);
        assert_eq!(pixel(&fb, 1, 1), [1, 1, 0, 0]);

        fb.resize(8, 4);
        assert_eq!(fb.byte_len(), 8 * 4 * fb.format().bpp() as usize);
        assert_eq!(pixel(&fb, 7, 3), [0, 0, 0, 0]);
    }

    /// A size change keeps the pixels that still fit, so a spurious size
    /// announcement cannot blank a desktop that was already painted.
    #[test]
    fn resize_keeps_the_overlapping_pixels() {
        let mut fb = grid();
        fb.resize(6, 6);
        assert_eq!(pixel(&fb, 3, 3), [3, 3, 0, 0]);
        assert_eq!(pixel(&fb, 5, 5), [0, 0, 0, 0]);

        fb.resize(2, 2);
        assert_eq!(pixel(&fb, 1, 1), [1, 1, 0, 0]);

        fb.resize(2, 2);
        assert_eq!(pixel(&fb, 1, 1), [1, 1, 0, 0]);
    }

    #[test]
    fn snapshot_matches_the_buffer() {
        let fb = grid();
        let mut out = Vec::new();
        fb.snapshot_into(&mut out);
        assert_eq!(out, fb.data);
        assert_eq!(out.len(), fb.byte_len());
    }

    /// A settled batch publishes at the floor; a stream that never settles is
    /// still published at the maximum interval instead of freezing.
    #[test]
    fn publish_prefers_settled_with_a_maximum_delay() {
        let settle = Duration::from_millis(6);
        assert!(!publish_due(settle, settle, Duration::ZERO));
        assert!(publish_due(settle, settle, PUBLISH_INTERVAL));

        assert!(!publish_due(settle, Duration::ZERO, PUBLISH_INTERVAL));
        assert!(!publish_due(settle, Duration::ZERO, MAX_PUBLISH_INTERVAL / 2));
        assert!(publish_due(settle, Duration::ZERO, MAX_PUBLISH_INTERVAL));
    }

    /// A pending request skips the idle interval instead of sleeping it out.
    #[test]
    fn poll_gate_wakes_on_request() {
        let gate = PollGate::new();
        gate.request();
        let start = Instant::now();
        gate.wait();
        assert!(start.elapsed() < DISCOVERY_INTERVAL / 2);
    }
}