use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};

use super::{Protocol, Frame};
use super::decklink;
use super::ndi;
use super::test;
#[cfg(target_os = "macos")]
use super::syphon;
#[cfg(target_os = "macos")]
use super::avfoundation;
#[cfg(target_os = "windows")]
use super::spout;

pub type SourceRef = String;

/// Identity of a live source in the registry
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceKey {
    pub protocol: Protocol,
    pub source_ref: SourceRef,
}

impl SourceKey {
    pub fn new(protocol: Protocol, source_ref: SourceRef) -> Self {
        Self { protocol, source_ref }
    }
}

// Runtime video source
pub trait VideoSource: Send + Sync {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame>;
    fn stats(&self) -> Arc<Mutex<SourceStats>>;
}

/// Protocol-specific source configuration stored in the config file.
///
/// `Unknown` doubles as the "no config" default (`Value::Null`) and as the
/// carrier for config JSON this platform cannot interpret: the original value
/// is kept untouched and written back on save, so a project round-trips to a
/// platform that does understand it.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceConfig {
    Test(test::TestSourceConfig),
    Ndi(ndi::NdiSourceConfig),
    Decklink(decklink::DecklinkSourceConfig),
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonSourceConfig),
    #[cfg(target_os = "macos")]
    AvFoundation(avfoundation::AvFoundationSourceConfig),
    #[cfg(target_os = "windows")]
    Spout(spout::SpoutSourceConfig),
    Unknown(serde_json::Value),
}

impl Default for SourceConfig {
    fn default() -> Self {
        SourceConfig::Unknown(serde_json::Value::Null)
    }
}

/// Known variants in their original wire format (internally tagged with
/// `protocol`); used by both directions of the custom serde impls below.
#[derive(Serialize, Deserialize)]
#[serde(tag = "protocol")]
enum SourceConfigInner {
    Test(test::TestSourceConfig),
    Ndi(ndi::NdiSourceConfig),
    Decklink(decklink::DecklinkSourceConfig),
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonSourceConfig),
    #[cfg(target_os = "macos")]
    AvFoundation(avfoundation::AvFoundationSourceConfig),
    #[cfg(target_os = "windows")]
    Spout(spout::SpoutSourceConfig),
}

impl From<SourceConfigInner> for SourceConfig {
    fn from(inner: SourceConfigInner) -> Self {
        match inner {
            SourceConfigInner::Test(c) => SourceConfig::Test(c),
            SourceConfigInner::Ndi(c) => SourceConfig::Ndi(c),
            SourceConfigInner::Decklink(c) => SourceConfig::Decklink(c),
            #[cfg(target_os = "macos")]
            SourceConfigInner::Syphon(c) => SourceConfig::Syphon(c),
            #[cfg(target_os = "macos")]
            SourceConfigInner::AvFoundation(c) => SourceConfig::AvFoundation(c),
            #[cfg(target_os = "windows")]
            SourceConfigInner::Spout(c) => SourceConfig::Spout(c),
        }
    }
}

impl Serialize for SourceConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            SourceConfig::Unknown(v) if v.is_null() => {
                // Historical wire form of a missing/default config.
                serde_json::json!({"protocol": "Unknown"}).serialize(serializer)
            }
            SourceConfig::Unknown(v) => v.serialize(serializer),
            SourceConfig::Test(c) => SourceConfigInner::Test(c.clone()).serialize(serializer),
            SourceConfig::Ndi(c) => SourceConfigInner::Ndi(c.clone()).serialize(serializer),
            SourceConfig::Decklink(c) => SourceConfigInner::Decklink(c.clone()).serialize(serializer),
            #[cfg(target_os = "macos")]
            SourceConfig::Syphon(c) => SourceConfigInner::Syphon(c.clone()).serialize(serializer),
            #[cfg(target_os = "macos")]
            SourceConfig::AvFoundation(c) => SourceConfigInner::AvFoundation(c.clone()).serialize(serializer),
            #[cfg(target_os = "windows")]
            SourceConfig::Spout(c) => SourceConfigInner::Spout(c.clone()).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for SourceConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_null() {
            return Ok(SourceConfig::default());
        }
        match serde_json::from_value::<SourceConfigInner>(value.clone()) {
            Ok(inner) => return Ok(inner.into()),
            // Unrecognized `protocol` tag (a protocol this platform does not
            // have, or a corrupt known one): keep the original JSON verbatim.
            Err(_) => return Ok(SourceConfig::Unknown(value)),
        }
    }
}

impl SourceConfig {
    /// Default config for a protocol, used when a source switches protocols
    /// (e.g. away from an unavailable one) and the old config no longer applies.
    pub fn for_protocol(protocol: &Protocol) -> Self {
        match protocol {
            Protocol::Test => SourceConfig::Test(test::TestSourceConfig::default()),
            Protocol::Ndi => SourceConfig::Ndi(ndi::NdiSourceConfig::default()),
            Protocol::Decklink => SourceConfig::Decklink(decklink::DecklinkSourceConfig::default()),
            #[cfg(target_os = "macos")]
            Protocol::Syphon => SourceConfig::Syphon(syphon::SyphonSourceConfig::default()),
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation => SourceConfig::AvFoundation(avfoundation::AvFoundationSourceConfig::default()),
            #[cfg(target_os = "windows")]
            Protocol::Spout => SourceConfig::Spout(spout::SpoutSourceConfig::default()),
            Protocol::Unknown(_) => SourceConfig::default(),
        }
    }
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

/// Data a source needs to open that cannot be derived from the `SourceKey`
/// alone. Never persisted — it lives only in the registry so respawn can
/// reopen the source without rediscovery.
#[derive(Clone, Debug)]
pub enum SourceRuntimeConfig {
    Test,
    Ndi { discovered: grafton_ndi::Source },
    Decklink { supported_connections: decklink::VideoConnections },
    #[cfg(target_os = "macos")]
    Syphon,
    #[cfg(target_os = "macos")]
    AvFoundation,
    #[cfg(target_os = "windows")]
    Spout,
}

#[allow(clippy::large_enum_variant)]
pub enum SourceKind {
    Test(test::TestSource, test::TestSourceConfig, SourceRuntimeConfig),
    Ndi(ndi::NdiSource, ndi::NdiSourceConfig, SourceRuntimeConfig),
    Decklink(decklink::DecklinkSource, decklink::DecklinkSourceConfig, SourceRuntimeConfig),
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonSource, syphon::SyphonSourceConfig, SourceRuntimeConfig),
    #[cfg(target_os = "macos")]
    AvFoundation(avfoundation::AvFoundationSource, avfoundation::AvFoundationSourceConfig, SourceRuntimeConfig),
    #[cfg(target_os = "windows")]
    Spout(spout::SpoutSource, spout::SpoutSourceConfig, SourceRuntimeConfig),
}

impl SourceKind {
    fn new_test(key: &SourceKey, config: test::TestSourceConfig) -> Self {
        let src = test::TestSource::spawn(key.source_ref.clone(), &config);
        return Self::Test(src, config, SourceRuntimeConfig::Test);
    }

    fn new_ndi(key: &SourceKey, config: ndi::NdiSourceConfig, discovered: grafton_ndi::Source) -> Self {
        let src = ndi::NdiSource::spawn(key.source_ref.clone(), discovered.clone(), &config);
        return Self::Ndi(src, config, SourceRuntimeConfig::Ndi { discovered });
    }

    fn new_decklink(key: &SourceKey, config: decklink::DecklinkSourceConfig, supported_connections: decklink::VideoConnections) -> Self {
        let src = decklink::DecklinkSource::spawn(key.source_ref.clone(), &config);
        return Self::Decklink(src, config, SourceRuntimeConfig::Decklink { supported_connections });
    }

    #[cfg(target_os = "macos")]
    fn new_syphon(key: &SourceKey, config: syphon::SyphonSourceConfig) -> Self {
        let src = syphon::SyphonSource::spawn(key.source_ref.clone());
        return Self::Syphon(src, config, SourceRuntimeConfig::Syphon);
    }

    #[cfg(target_os = "macos")]
    fn new_avfoundation(key: &SourceKey, config: avfoundation::AvFoundationSourceConfig) -> Self {
        let src = avfoundation::AvFoundationSource::spawn(key.source_ref.clone(), &config);
        return Self::AvFoundation(src, config, SourceRuntimeConfig::AvFoundation);
    }

    #[cfg(target_os = "windows")]
    fn new_spout(key: &SourceKey, config: spout::SpoutSourceConfig) -> Self {
        let src = spout::SpoutSource::spawn(key.source_ref.clone());
        return Self::Spout(src, config, SourceRuntimeConfig::Spout);
    }

    /// Rebuild this source under the same key from the config and runtime data
    /// it already carries — neither can be derived from the key alone.
    fn respawn(self, key: &SourceKey) -> Self {
        return match self {
            Self::Test(_, config, SourceRuntimeConfig::Test) => Self::new_test(key, config),
            Self::Ndi(_, config, SourceRuntimeConfig::Ndi { discovered }) => Self::new_ndi(key, config, discovered),
            Self::Decklink(_, config, SourceRuntimeConfig::Decklink { supported_connections }) => Self::new_decklink(key, config, supported_connections),
            #[cfg(target_os = "macos")]
            Self::Syphon(_, config, SourceRuntimeConfig::Syphon) => Self::new_syphon(key, config),
            #[cfg(target_os = "macos")]
            Self::AvFoundation(_, config, SourceRuntimeConfig::AvFoundation) => Self::new_avfoundation(key, config),
            #[cfg(target_os = "windows")]
            Self::Spout(_, config, SourceRuntimeConfig::Spout) => Self::new_spout(key, config),
            // Every kind is constructed with its own runtime variant; no other
            // pairing can exist.
            _ => unreachable!("source kind paired with a foreign runtime config"),
        };
    }

    pub fn latest(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Frame> {
        match self {
            SourceKind::Test(s, _, _) => s.latest(device, queue),
            SourceKind::Ndi(s, _, _) => s.latest(device, queue),
            SourceKind::Decklink(s, _, _) => s.latest(device, queue),
            #[cfg(target_os = "macos")]
            SourceKind::Syphon(s, _, _) => s.latest(device, queue),
            #[cfg(target_os = "macos")]
            SourceKind::AvFoundation(s, _, _) => s.latest(device, queue),
            #[cfg(target_os = "windows")]
            SourceKind::Spout(s, _, _) => s.latest(device, queue),
        }
    }

    pub fn stats(&self) -> Arc<Mutex<SourceStats>> {
        match self {
            SourceKind::Test(s, _, _) => s.stats(),
            SourceKind::Ndi(s, _, _) => s.stats(),
            SourceKind::Decklink(s, _, _) => s.stats(),
            #[cfg(target_os = "macos")]
            SourceKind::Syphon(s, _, _) => s.stats(),
            #[cfg(target_os = "macos")]
            SourceKind::AvFoundation(s, _, _) => s.stats(),
            #[cfg(target_os = "windows")]
            SourceKind::Spout(s, _, _) => s.stats(),
        }
    }

    pub fn to_config(&self) -> SourceConfig {
        match self {
            SourceKind::Test(_, cfg, _) => SourceConfig::Test(cfg.clone()),
            SourceKind::Ndi(_, cfg, _) => SourceConfig::Ndi(cfg.clone()),
            SourceKind::Decklink(_, cfg, _) => SourceConfig::Decklink(cfg.clone()),
            #[cfg(target_os = "macos")]
            SourceKind::Syphon(_, cfg, _) => SourceConfig::Syphon(cfg.clone()),
            #[cfg(target_os = "macos")]
            SourceKind::AvFoundation(_, cfg, _) => SourceConfig::AvFoundation(cfg.clone()),
            #[cfg(target_os = "windows")]
            SourceKind::Spout(_, cfg, _) => SourceConfig::Spout(cfg.clone()),
        }
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
    pub key: SourceKey,
    pub kind: SourceKind,
}

// Registry of all live sources. Owned by the UI thread; sources render on their own threads.
pub struct SourceRegistry {
    sources: HashMap<SourceKey, SourceKind>,
    next_test: u32,
    pending_restarts: HashSet<SourceKey>,
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

    /// Register a live source under its key. A key that is already live or
    /// mid-restart is left alone, so every quad binding it shares one receiver.
    fn add(&mut self, key: SourceKey, kind: SourceKind) -> SourceKey {
        if !self.contains(&key) {
            self.sources.insert(key.clone(), kind);
        }
        return key;
    }

    /// Create a dedicated test source under a fresh key.
    pub fn add_test(&mut self, config: Option<test::TestSourceConfig>) -> SourceKey {
        let key = loop {
            self.next_test += 1;
            let letter = (b'A' + (self.next_test as u8 - 1) % 26) as char;
            // Past 26 live test sources the letter wraps; the counter suffix
            // keeps the key unique so a new source never overwrites a live one.
            let source_ref = if self.next_test <= 26 {
                format!("Test {letter}")
            } else {
                format!("Test {letter} {}", self.next_test)
            };
            let key = SourceKey::new(Protocol::Test, source_ref);
            if !self.contains(&key) {
                break key;
            }
        };
        let kind = SourceKind::new_test(&key, config.unwrap_or_default());
        return self.add(key, kind);
    }

    pub fn add_ndi(&mut self, key: SourceKey, config: ndi::NdiSourceConfig, discovered: grafton_ndi::Source) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        self.sources.insert(key.clone(), SourceKind::new_ndi(&key, config, discovered));
        return key;
    }

    pub fn add_decklink(&mut self, key: SourceKey, config: decklink::DecklinkSourceConfig, supported_connections: decklink::VideoConnections) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        self.sources.insert(
            key.clone(),
            SourceKind::new_decklink(&key, config, supported_connections),
        );
        return key;
    }

    #[cfg(target_os = "macos")]
    pub fn add_syphon(&mut self, key: SourceKey, config: syphon::SyphonSourceConfig) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        self.sources.insert(key.clone(), SourceKind::new_syphon(&key, config));
        return key;
    }

    #[cfg(target_os = "macos")]
    pub fn add_avfoundation(&mut self, key: SourceKey, config: avfoundation::AvFoundationSourceConfig) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        self.sources.insert(key.clone(), SourceKind::new_avfoundation(&key, config));
        return key;
    }

    #[cfg(target_os = "windows")]
    pub fn add_spout(&mut self, key: SourceKey, config: spout::SpoutSourceConfig) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        self.sources.insert(key.clone(), SourceKind::new_spout(&key, config));
        return key;
    }

    /// Whether a source for this key is live or being restarted.
    pub fn contains(&self, key: &SourceKey) -> bool {
        self.sources.contains_key(key) || self.pending_restarts.contains(key)
    }

    pub fn get(&self, key: &SourceKey) -> Option<&SourceKind> {
        self.sources.get(key)
    }

    pub fn get_mut(&mut self, key: &SourceKey) -> Option<&mut SourceKind> {
        self.sources.get_mut(key)
    }

    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = (&SourceKey, &SourceKind)> {
        self.sources.iter()
    }

    #[allow(dead_code)]
    pub fn remove(&mut self, key: &SourceKey) {
        self.sources.remove(key);
    }

    /// Restart the source under `key` with its current config. Test and Spout
    /// rebuild inline (no blocking work); every other protocol reopens a device
    /// or receiver on a thread, and the key stays pending until
    /// `apply_pending_restarts` reinserts the new source — quads show a
    /// placeholder meanwhile.
    pub fn restart(&mut self, key: &SourceKey) {
        let Some(kind) = self.sources.remove(key) else {
            return;
        };
        match kind {
            kind @ SourceKind::Test(..) => {
                self.sources.insert(key.clone(), kind.respawn(key));
            }
            #[cfg(target_os = "windows")]
            kind @ SourceKind::Spout(..) => {
                self.sources.insert(key.clone(), kind.respawn(key));
            }
            kind => {
                let key = key.clone();
                self.pending_restarts.insert(key.clone());
                let tx = self.restart_tx.clone();
                let thread_name =
                    format!("{}-restart-{}", key.protocol.name().to_lowercase(), key.source_ref);
                std::thread::Builder::new()
                    .name(thread_name)
                    .spawn(move || {
                        let kind = kind.respawn(&key);
                        let _ = tx.send(RestartResult { key, kind });
                    })
                    .expect("spawn source restart thread");
            }
        }
    }

    pub fn apply_pending_restarts(&mut self) {
        while let Ok(result) = self.restart_rx.try_recv() {
            self.pending_restarts.remove(&result.key);
            self.sources.insert(result.key, result.kind);
        }
    }

    /// Remove all sources not referenced anymore.
    pub fn cleanup_orphaned_sources(&mut self, active_keys: &[SourceKey]) {
        let active: HashSet<&SourceKey> = active_keys.iter().collect();
        let to_remove: Vec<SourceKey> = self
            .sources
            .keys()
            .filter(|key| !active.contains(key))
            .cloned()
            .collect();
        for key in to_remove {
            self.sources.remove(&key);
        }
    }

    pub fn list_sources(&self, protocol: Protocol) -> Vec<(&SourceKey, &SourceKind)> {
        self.sources
            .iter()
            .filter(|(key, _)| key.protocol == protocol)
            .collect()
    }

    pub fn clear(&mut self) {
        self.sources.clear();
        self.next_test = 0;
        self.pending_restarts.clear();
        while self.restart_rx.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A source_ref alone is not a registry identity: two protocols may carry
    /// the same reference without one lookup hitting the other's source.
    #[test]
    fn keys_are_protocol_scoped() {
        let mut registry = SourceRegistry::new();
        let key = SourceKey::new(Protocol::Test, "Camera".into());
        registry.add(key.clone(), SourceKind::new_test(&key, Default::default()));

        assert!(registry.contains(&key));
        let other_protocol = SourceKey::new(Protocol::Ndi, "Camera".into());
        assert!(!registry.contains(&other_protocol));
        assert!(registry.get(&other_protocol).is_none());
        assert_eq!(registry.list_sources(Protocol::Test).len(), 1);
        assert_eq!(registry.list_sources(Protocol::Ndi).len(), 0);
    }

    /// A second quad binding the same protocol + source_ref shares the one
    /// live receiver instead of spawning another.
    #[test]
    fn duplicate_key_shares_one_runtime_source() {
        let mut registry = SourceRegistry::new();
        let key = registry.add_test(None);
        let count = registry.iter().count();

        let again = registry.add(key.clone(), SourceKind::new_test(&key, Default::default()));
        assert_eq!(again, key);
        assert_eq!(registry.iter().count(), count);
        assert!(registry.get(&key).is_some());
    }

    /// Past 26 test sources the letter suffix wraps; keys must stay unique so
    /// a new source never overwrites a live one.
    #[test]
    fn add_test_keys_stay_unique_past_the_letter_wrap() {
        let mut registry = SourceRegistry::new();
        let config = test::TestSourceConfig {
            width: 64,
            height: 64,
            ..Default::default()
        };
        let mut keys = HashSet::new();
        for _ in 0..30 {
            let key = registry.add_test(Some(config.clone()));
            assert!(keys.insert(key.clone()), "duplicate key {:?}", key);
        }
        assert_eq!(registry.iter().count(), 30);
    }

    /// A restart rebuilds the source under the same key.
    #[test]
    fn restart_keeps_the_key() {
        let mut registry = SourceRegistry::new();
        let key = registry.add_test(None);
        registry.restart(&key);
        assert!(registry.get(&key).is_some());
    }
}
