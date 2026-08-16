use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub mod decklink;
pub mod ndi;
pub mod test;

#[cfg(target_os = "macos")]
pub mod syphon;

pub use decklink::source::{DecklinkConfig, DecklinkSource};
pub use ndi::source::{NdiConfig, NdiSource};
pub use test::{TestConfig, TestSource};

#[cfg(target_os = "macos")]
pub use syphon::source::{SyphonConfig, SyphonSource};

pub type SourceId = String;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    Bgra8,
    /// Packed YUV 4:2:2 (UYVY), 2 bytes per pixel.
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

const FPS_WINDOW: Duration = Duration::from_secs(2);
const TIMING_WINDOW: usize = 60;
const MAX_FRAME_TIMESTAMPS: usize = 256;

#[derive(Debug, Clone)]
pub struct SourceStats {
    pub width: u32,
    pub height: u32,
    pub pixel_format: String,
    pub nominal_fps: f64,
    pub frames_received: u64,
    pub frames_presented: u64,
    pub frames_dropped: u64,
    pub copy_time_ms: f32,
    pub upload_time_ms: f32,
    pub computed_fps: f64,
    recent_frames: VecDeque<Instant>,
    copy_times: VecDeque<f32>,
    upload_times: VecDeque<f32>,
}

impl Default for SourceStats {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            pixel_format: String::new(),
            nominal_fps: 0.0,
            frames_received: 0,
            frames_presented: 0,
            frames_dropped: 0,
            copy_time_ms: 0.0,
            upload_time_ms: 0.0,
            computed_fps: 0.0,
            recent_frames: VecDeque::new(),
            copy_times: VecDeque::new(),
            upload_times: VecDeque::new(),
        }
    }
}

impl SourceStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_frame(&mut self, width: u32, height: u32, pixel_format: &str, nominal_fps: f64) {
        let now = Instant::now();
        self.width = width;
        self.height = height;
        self.pixel_format = pixel_format.to_string();
        self.nominal_fps = nominal_fps;
        self.frames_received += 1;
        self.recent_frames.push_back(now);
        while let Some(front) = self.recent_frames.front() {
            if now.duration_since(*front) > FPS_WINDOW {
                self.recent_frames.pop_front();
            } else {
                break;
            }
        }
        if self.recent_frames.len() > MAX_FRAME_TIMESTAMPS {
            self.recent_frames.pop_front();
        }
        if self.recent_frames.len() >= 2 {
            let duration = now.duration_since(*self.recent_frames.front().unwrap());
            let secs = duration.as_secs_f64();
            if secs > 0.0 {
                self.computed_fps = (self.recent_frames.len() - 1) as f64 / secs;
            }
        }
    }

    pub fn record_copy_time(&mut self, ms: f32) {
        self.copy_times.push_back(ms);
        if self.copy_times.len() > TIMING_WINDOW {
            self.copy_times.pop_front();
        }
        self.copy_time_ms = self.average(&self.copy_times);
    }

    pub fn record_upload_time(&mut self, ms: f32) {
        self.upload_times.push_back(ms);
        if self.upload_times.len() > TIMING_WINDOW {
            self.upload_times.pop_front();
        }
        self.upload_time_ms = self.average(&self.upload_times);
        self.frames_presented += 1;
    }

    pub fn record_dropped(&mut self, count: u64) {
        self.frames_dropped += count;
    }

    fn average(&self, values: &VecDeque<f32>) -> f32 {
        if values.is_empty() {
            return 0.0;
        }
        values.iter().sum::<f32>() / values.len() as f32
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

pub trait VideoSource: Send + Sync {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame>;
    #[allow(dead_code)]
    fn name(&self) -> &str;
    fn stats(&self) -> Arc<Mutex<SourceStats>>;
}

/// Uniform block consumed by the compositor's fragment shader.
/// Must stay in sync with the `ConvUniform` struct in `compositor.rs`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ConvUniform {
    pub mode: u32, // 0 = passthrough, 1 = UYVY BT.601, 2 = UYVY BT.709
    pub width: f32,
    pub height: f32,
    pub _pad: f32,
}

pub enum SourceKind {
    Test(TestSource, TestConfig),
    Ndi(NdiSource, NdiConfig, grafton_ndi::Source),
    #[cfg(target_os = "macos")]
    Syphon(SyphonSource, SyphonConfig, String),
    Decklink(DecklinkSource, DecklinkConfig, String),
}

impl SourceKind {
    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        match self {
            SourceKind::Test(s, _) => s.name(),
            SourceKind::Ndi(s, _, _) => s.name(),
            #[cfg(target_os = "macos")]
            SourceKind::Syphon(s, _, _) => s.name(),
            SourceKind::Decklink(s, _, _) => s.name(),
        }
    }

    pub fn latest(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Frame> {
        match self {
            SourceKind::Test(s, _) => s.latest(device, queue),
            SourceKind::Ndi(s, _, _) => s.latest(device, queue),
            #[cfg(target_os = "macos")]
            SourceKind::Syphon(s, _, _) => s.latest(device, queue),
            SourceKind::Decklink(s, _, _) => s.latest(device, queue),
        }
    }

    pub fn stats(&self) -> Arc<Mutex<SourceStats>> {
        match self {
            SourceKind::Test(s, _) => s.stats(),
            SourceKind::Ndi(s, _, _) => s.stats(),
            #[cfg(target_os = "macos")]
            SourceKind::Syphon(s, _, _) => s.stats(),
            SourceKind::Decklink(s, _, _) => s.stats(),
        }
    }

    pub fn is_test(&self) -> bool {
        matches!(self, SourceKind::Test(_, _))
    }

    pub fn is_syphon(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            matches!(self, SourceKind::Syphon(_, _, _))
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn is_decklink(&self) -> bool {
        matches!(self, SourceKind::Decklink(_, _, _))
    }

    #[allow(dead_code)]
    pub fn test_config_mut(&mut self) -> Option<&mut TestConfig> {
        match self {
            SourceKind::Test(_, cfg) => Some(cfg),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn ndi_config_mut(&mut self) -> Option<&mut NdiConfig> {
        match self {
            SourceKind::Ndi(_, cfg, _) => Some(cfg),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn syphon_config_mut(&mut self) -> Option<&mut SyphonConfig> {
        #[cfg(target_os = "macos")]
        {
            match self {
                SourceKind::Syphon(_, cfg, _) => Some(cfg),
                _ => None,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    #[allow(dead_code)]
    pub fn decklink_config_mut(&mut self) -> Option<&mut DecklinkConfig> {
        match self {
            SourceKind::Decklink(_, cfg, _) => Some(cfg),
            _ => None,
        }
    }
}

/// All live sources. Owned by the UI thread; sources render on their own threads.
pub struct Registry {
    sources: HashMap<SourceId, SourceKind>,
    next_test: u32,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
            next_test: 0,
        }
    }

    /// Create a dedicated test source for a layer.
    pub fn add_test(&mut self) -> SourceId {
        self.next_test += 1;
        let letter = (b'A' + (self.next_test as u8 - 1) % 26) as char;
        let id = format!("Test {letter}");
        let src = TestSource::spawn(id.clone(), self.next_test);
        self.sources
            .insert(id.clone(), SourceKind::Test(src, TestConfig::default()));
        id
    }

    pub fn add_ndi(&mut self, name: String, source: grafton_ndi::Source) -> SourceId {
        if self.sources.contains_key(&name) {
            return name;
        }
        let cfg = NdiConfig::default();
        let src = NdiSource::spawn(name.clone(), source.clone(), &cfg);
        self.sources
            .insert(name.clone(), SourceKind::Ndi(src, cfg, source));
        name
    }

    #[cfg(target_os = "macos")]
    pub fn add_syphon(&mut self, name: String, server_name: String) -> SourceId {
        if self.sources.contains_key(&name) {
            return name;
        }
        let src = SyphonSource::spawn(name.clone(), server_name);
        self.sources.insert(
            name.clone(),
            SourceKind::Syphon(src, SyphonConfig::default(), name.clone()),
        );
        name
    }

    pub fn add_decklink(
        &mut self,
        name: String,
        display_name: String,
        supported_connections: Option<String>,
    ) -> SourceId {
        if self.sources.contains_key(&name) {
            return name;
        }
        let mut cfg = DecklinkConfig::default();
        if let Some(conn) = supported_connections {
            cfg.supported_connections = conn;
        }
        let src = DecklinkSource::spawn(name.clone(), display_name, &cfg);
        self.sources
            .insert(name.clone(), SourceKind::Decklink(src, cfg, name.clone()));
        name
    }

    pub fn get(&self, id: &SourceId) -> Option<&SourceKind> {
        self.sources.get(id)
    }

    pub fn get_mut(&mut self, id: &SourceId) -> Option<&mut SourceKind> {
        self.sources.get_mut(id)
    }

    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = (&SourceId, &SourceKind)> {
        self.sources.iter()
    }

    #[allow(dead_code)]
    pub fn remove(&mut self, id: &SourceId) {
        self.sources.remove(id);
    }

    /// Restart an NDI source with its current config.
    pub fn restart_ndi(&mut self, name: &str) {
        if let Some(SourceKind::Ndi(_, cfg, source)) = self.sources.remove(name) {
            let new = NdiSource::spawn(name.to_string(), source.clone(), &cfg);
            self.sources
                .insert(name.to_string(), SourceKind::Ndi(new, cfg, source));
        }
    }

    #[cfg(target_os = "macos")]
    pub fn restart_syphon(&mut self, name: &str) {
        if let Some(SourceKind::Syphon(_, cfg, server_name)) = self.sources.remove(name) {
            let new = SyphonSource::spawn(name.to_string(), server_name);
            self.sources.insert(
                name.to_string(),
                SourceKind::Syphon(new, cfg, name.to_string()),
            );
        }
    }

    pub fn restart_decklink(&mut self, name: &str) {
        if let Some(SourceKind::Decklink(_, cfg, display_name)) = self.sources.remove(name) {
            let new = DecklinkSource::spawn(name.to_string(), display_name, &cfg);
            self.sources.insert(
                name.to_string(),
                SourceKind::Decklink(new, cfg, name.to_string()),
            );
        }
    }

    /// Remove all sources not referenced by any layer.
    pub fn cleanup_orphaned_sources(&mut self, active_source_ids: &[&str]) {
        let active: HashSet<&str> = active_source_ids.iter().copied().collect();
        let to_remove: Vec<String> = self
            .sources
            .iter()
            .filter(|(id, _)| !active.contains(id.as_str()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in to_remove {
            self.sources.remove(&id);
        }
    }

    pub fn list_test_sources(&self) -> Vec<(&SourceId, &SourceKind)> {
        self.sources.iter().filter(|(_, sk)| sk.is_test()).collect()
    }

    pub fn list_ndi_sources(&self) -> Vec<(&SourceId, &SourceKind)> {
        self.sources
            .iter()
            .filter(|(_, sk)| !sk.is_test() && !sk.is_syphon() && !sk.is_decklink())
            .collect()
    }

    #[cfg(target_os = "macos")]
    pub fn list_syphon_sources(&self) -> Vec<(&SourceId, &SourceKind)> {
        self.sources
            .iter()
            .filter(|(_, sk)| sk.is_syphon())
            .collect()
    }

    pub fn list_decklink_sources(&self) -> Vec<(&SourceId, &SourceKind)> {
        self.sources
            .iter()
            .filter(|(_, sk)| sk.is_decklink())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::SourceStats;

    #[test]
    fn stats_records_and_averages() {
        let mut s = SourceStats::new();
        s.record_frame(1920, 1080, "BGRA8", 30.0);
        s.record_copy_time(1.0);
        s.record_copy_time(3.0);
        s.record_upload_time(2.0);
        s.record_upload_time(4.0);
        s.record_dropped(1);
        assert_eq!(s.width, 1920);
        assert_eq!(s.height, 1080);
        assert_eq!(s.pixel_format, "BGRA8");
        assert!((s.nominal_fps - 30.0).abs() < 0.001);
        assert_eq!(s.frames_received, 1);
        assert_eq!(s.frames_presented, 2);
        assert_eq!(s.frames_dropped, 1);
        assert!((s.copy_time_ms - 2.0).abs() < 0.001);
        assert!((s.upload_time_ms - 3.0).abs() < 0.001);
    }
}
