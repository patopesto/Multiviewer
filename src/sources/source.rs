use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};

use super::{Protocol, Frame};
use super::decklink::{self, VideoConnections};
use super::ndi;
use super::test;
#[cfg(target_os = "macos")]
use super::syphon;

pub type SourceId = String;

// Runtime video source
pub trait VideoSource: Send + Sync {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame>;
    #[allow(dead_code)]
    fn name(&self) -> &str;
    fn stats(&self) -> Arc<Mutex<SourceStats>>;
}

/// Protocol-specific source configuration stored in the config file.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(tag = "protocol")]
pub enum SourceConfig {
    Test(test::TestSourceConfig),
    Ndi(ndi::NdiSourceConfig),
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonSourceConfig),
    Decklink(decklink::DecklinkSourceConfig),
    #[default]
    #[serde(other)]
    Unknown,
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
    Test(test::TestSource, test::TestSourceConfig),
    Ndi(ndi::NdiSource, ndi::NdiSourceConfig, grafton_ndi::Source),
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonSource, syphon::SyphonSourceConfig, String),
    Decklink(decklink::DecklinkSource, decklink::DecklinkSourceConfig, String),
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

    pub fn protocol(&self) -> Protocol {
        match self {
            SourceKind::Test(_, _) => Protocol::Test,
            SourceKind::Ndi(_, _, _) => Protocol::Ndi,
            #[cfg(target_os = "macos")]
            SourceKind::Syphon(_, _, _) => Protocol::Syphon,
            SourceKind::Decklink(_, _, _) => Protocol::Decklink,
        }
    }

    pub fn to_config(&self) -> SourceConfig {
        match self {
            SourceKind::Test(_, cfg) => SourceConfig::Test(cfg.clone()),
            SourceKind::Ndi(_, cfg, _) => SourceConfig::Ndi(cfg.clone()),
            #[cfg(target_os = "macos")]
            SourceKind::Syphon(_, cfg, _) => SourceConfig::Syphon(cfg.clone()),
            SourceKind::Decklink(_, cfg, _) => SourceConfig::Decklink(cfg.clone()),
        }
    }
}

impl From<&SourceKind> for Protocol {
    fn from(kind: &SourceKind) -> Self {
        kind.protocol()
    }
}


// Stats
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

pub struct RestartResult {
    pub id: SourceId,
    pub kind: SourceKind,
}

// Registry of all live sources. Owned by the UI thread; sources render on their own threads.
pub struct SourceRegistry {
    sources: HashMap<SourceId, SourceKind>,
    next_test: u32,
    pending_restarts: HashSet<SourceId>,
    restart_tx: std::sync::mpsc::Sender<RestartResult>,
    restart_rx: std::sync::mpsc::Receiver<RestartResult>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            sources: HashMap::new(),
            next_test: 0,
            pending_restarts: HashSet::new(),
            restart_tx: tx,
            restart_rx: rx,
        }
    }

    /// Create a dedicated test source.
    pub fn add_test(&mut self, config: Option<test::TestSourceConfig>) -> SourceId {
        self.next_test += 1;
        let letter = (b'A' + (self.next_test as u8 - 1) % 26) as char;
        let id = format!("Test {letter}");
        let cfg = config.unwrap_or_default();
        let src = test::TestSource::spawn(id.clone(), &cfg);
        self.sources.insert(id.clone(), SourceKind::Test(src, cfg));
        return id;
    }

    pub fn add_ndi(&mut self, id: SourceId, source: grafton_ndi::Source, config: Option<ndi::NdiSourceConfig>) -> SourceId {
        if self.sources.contains_key(&id) {
            return id;
        }
        let cfg = config.unwrap_or_default();
        let src = ndi::NdiSource::spawn(id.clone(), source.clone(), &cfg);
        self.sources.insert(id.clone(), SourceKind::Ndi(src, cfg, source));
        return id;
    }

    #[cfg(target_os = "macos")]
    pub fn add_syphon(&mut self, id: SourceId, server_name: String, _config: Option<syphon::SyphonSourceConfig>) -> SourceId {
        if self.sources.contains_key(&id) {
            return id;
        }
        let src = syphon::SyphonSource::spawn(id.clone(), server_name);
        self.sources.insert(id.clone(), SourceKind::Syphon(src, syphon::SyphonSourceConfig::default(), id.clone()));
        return id;
    }

    pub fn add_decklink(
        &mut self,
        id: SourceId,
        display_name: String,
        supported_connections: Option<VideoConnections>,
        config: Option<decklink::DecklinkSourceConfig>,
    ) -> SourceId {
        if self.sources.contains_key(&id) || self.pending_restarts.contains(&id) {
            return id;
        }
        let supported = supported_connections.unwrap_or(VideoConnections::EMPTY);
        let mut cfg = config.unwrap_or_default();
        cfg.supported_connections = supported;
        let src = decklink::DecklinkSource::spawn(id.clone(), display_name, &cfg);
        self.sources.insert(id.clone(), SourceKind::Decklink(src, cfg, id.clone()));
        return id;
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

     pub fn restart(&mut self, id: &str) {
        let protocol = self.sources.get(id).map(|s| s.protocol());
        match protocol {
            Some(Protocol::Test) => self.restart_test(id),
            Some(Protocol::Ndi) => self.restart_ndi(id),
            Some(Protocol::Decklink) => self.restart_decklink(id),
            #[cfg(target_os = "macos")]
            Some(Protocol::Syphon) => self.restart_syphon(id),
            None => {}
        }
    }

    /// Restart a test source with its current config.
    pub fn restart_test(&mut self, id: &str) {
        if let Some(SourceKind::Test(_, cfg)) = self.sources.remove(id) {
            let src = test::TestSource::spawn(id.to_string(), &cfg);
            self.sources.insert(id.to_string(), SourceKind::Test(src, cfg));
        }
    }

    /// Restart an NDI source with its current config.
    pub fn restart_ndi(&mut self, id: &str) {
        if let Some(SourceKind::Ndi(_, cfg, source)) = self.sources.remove(id) {
            self.pending_restarts.insert(id.to_string());
            let tx = self.restart_tx.clone();
            let id = id.to_string();
            std::thread::Builder::new()
                .name(format!("ndi-restart-{id}"))
                .spawn(move || {
                    let new = ndi::NdiSource::spawn(id.clone(), source.clone(), &cfg);
                    let _ = tx.send(RestartResult {
                        id: id.clone(),
                        kind: SourceKind::Ndi(new, cfg, source),
                    });
                })
                .expect("spawn ndi restart thread");
        }
    }

    #[cfg(target_os = "macos")]
    pub fn restart_syphon(&mut self, id: &str) {
        if let Some(SourceKind::Syphon(_, cfg, server_name)) = self.sources.remove(id) {
            self.pending_restarts.insert(id.to_string());
            let tx = self.restart_tx.clone();
            let id = id.to_string();
            let server = server_name.clone();
            std::thread::Builder::new()
                .name(format!("syphon-restart-{id}"))
                .spawn(move || {
                    let new = syphon::SyphonSource::spawn(id.clone(), server);
                    let _ = tx.send(RestartResult {
                        id: id.clone(),
                        kind: SourceKind::Syphon(new, cfg, id),
                    });
                })
                .expect("spawn syphon restart thread");
        }
    }

    pub fn restart_decklink(&mut self, id: &str) {
        if let Some(SourceKind::Decklink(old_source, cfg, display_name)) = self.sources.remove(id) {
            self.pending_restarts.insert(id.to_string());
            let tx = self.restart_tx.clone();
            let id = id.to_string();
            std::thread::Builder::new()
                .name(format!("decklink-restart-{id}"))
                .spawn(move || {
                    drop(old_source);
                    let new_source = decklink::DecklinkSource::spawn(id.clone(), display_name, &cfg);
                    let _ = tx.send(RestartResult {
                        id: id.clone(),
                        kind: SourceKind::Decklink(new_source, cfg, id),
                    });
                })
                .expect("spawn decklink restart thread");
        }
    }

    pub fn apply_pending_restarts(&mut self) {
        while let Ok(result) = self.restart_rx.try_recv() {
            self.pending_restarts.remove(&result.id);
            self.sources.insert(result.id, result.kind);
        }
    }

    /// Remove all sources not referenced anymore.
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

    pub fn list_sources(&self, protocol: Protocol) -> Vec<(&SourceId, &SourceKind)> {
        self.sources
            .iter()
            .filter(|(_, sk)| sk.protocol() == protocol)
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
