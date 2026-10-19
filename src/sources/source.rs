use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};

use super::{Protocol, Frame, NdiSourceInfo, OmtSourceInfo, SourceStats};
use super::decklink;
use super::ndi;
use super::omt;
use super::test;
use super::vnc;
use super::rdp;
#[cfg(target_os = "macos")]
use super::syphon;
#[cfg(target_os = "macos")]
use super::SyphonServerInfo;
#[cfg(target_os = "macos")]
use super::avfoundation;
#[cfg(target_os = "macos")]
use super::screencapturekit;
#[cfg(target_os = "windows")]
use super::spout;
#[cfg(target_os = "windows")]
use super::mediafoundation;
#[cfg(target_os = "windows")]
use super::directshow;
#[cfg(target_os = "windows")]
use super::windowscapture;

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
    Omt(omt::OmtSourceConfig),
    Decklink(decklink::DecklinkSourceConfig),
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonSourceConfig),
    #[cfg(target_os = "macos")]
    AvFoundation(avfoundation::AvFoundationSourceConfig),
    #[cfg(target_os = "macos")]
    ScreenCaptureKit(screencapturekit::ScreenCaptureKitSourceConfig),
    #[cfg(target_os = "windows")]
    Spout(spout::SpoutSourceConfig),
    #[cfg(target_os = "windows")]
    MediaFoundation(mediafoundation::MediaFoundationSourceConfig),
    #[cfg(target_os = "windows")]
    DirectShow(directshow::DirectShowSourceConfig),
    #[cfg(target_os = "windows")]
    WindowsCapture(windowscapture::WindowsCaptureSourceConfig),
    Vnc(vnc::VncSourceConfig),
    Rdp(rdp::RdpSourceConfig),
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
    Omt(omt::OmtSourceConfig),
    Decklink(decklink::DecklinkSourceConfig),
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonSourceConfig),
    #[cfg(target_os = "macos")]
    AvFoundation(avfoundation::AvFoundationSourceConfig),
    #[cfg(target_os = "macos")]
    ScreenCaptureKit(screencapturekit::ScreenCaptureKitSourceConfig),
    #[cfg(target_os = "windows")]
    Spout(spout::SpoutSourceConfig),
    #[cfg(target_os = "windows")]
    MediaFoundation(mediafoundation::MediaFoundationSourceConfig),
    #[cfg(target_os = "windows")]
    DirectShow(directshow::DirectShowSourceConfig),
    #[cfg(target_os = "windows")]
    WindowsCapture(windowscapture::WindowsCaptureSourceConfig),
    Vnc(vnc::VncSourceConfig),
    Rdp(rdp::RdpSourceConfig),
}

impl From<SourceConfigInner> for SourceConfig {
    fn from(inner: SourceConfigInner) -> Self {
        match inner {
            SourceConfigInner::Test(c) => SourceConfig::Test(c),
            SourceConfigInner::Ndi(c) => SourceConfig::Ndi(c),
            SourceConfigInner::Omt(c) => SourceConfig::Omt(c),
            SourceConfigInner::Decklink(c) => SourceConfig::Decklink(c),
            #[cfg(target_os = "macos")]
            SourceConfigInner::Syphon(c) => SourceConfig::Syphon(c),
            #[cfg(target_os = "macos")]
            SourceConfigInner::AvFoundation(c) => SourceConfig::AvFoundation(c),
            #[cfg(target_os = "macos")]
            SourceConfigInner::ScreenCaptureKit(c) => SourceConfig::ScreenCaptureKit(c),
            #[cfg(target_os = "windows")]
            SourceConfigInner::Spout(c) => SourceConfig::Spout(c),
            #[cfg(target_os = "windows")]
            SourceConfigInner::MediaFoundation(c) => SourceConfig::MediaFoundation(c),
            #[cfg(target_os = "windows")]
            SourceConfigInner::DirectShow(c) => SourceConfig::DirectShow(c),
            #[cfg(target_os = "windows")]
            SourceConfigInner::WindowsCapture(c) => SourceConfig::WindowsCapture(c),
            SourceConfigInner::Vnc(c) => SourceConfig::Vnc(c),
            SourceConfigInner::Rdp(c) => SourceConfig::Rdp(c),
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
            SourceConfig::Omt(c) => SourceConfigInner::Omt(c.clone()).serialize(serializer),
            SourceConfig::Decklink(c) => SourceConfigInner::Decklink(c.clone()).serialize(serializer),
            #[cfg(target_os = "macos")]
            SourceConfig::Syphon(c) => SourceConfigInner::Syphon(c.clone()).serialize(serializer),
            #[cfg(target_os = "macos")]
            SourceConfig::AvFoundation(c) => SourceConfigInner::AvFoundation(c.clone()).serialize(serializer),
            #[cfg(target_os = "macos")]
            SourceConfig::ScreenCaptureKit(c) => SourceConfigInner::ScreenCaptureKit(c.clone()).serialize(serializer),
            #[cfg(target_os = "windows")]
            SourceConfig::Spout(c) => SourceConfigInner::Spout(c.clone()).serialize(serializer),
            #[cfg(target_os = "windows")]
            SourceConfig::MediaFoundation(c) => SourceConfigInner::MediaFoundation(c.clone()).serialize(serializer),
            #[cfg(target_os = "windows")]
            SourceConfig::DirectShow(c) => SourceConfigInner::DirectShow(c.clone()).serialize(serializer),
            #[cfg(target_os = "windows")]
            SourceConfig::WindowsCapture(c) => SourceConfigInner::WindowsCapture(c.clone()).serialize(serializer),
            SourceConfig::Vnc(c) => SourceConfigInner::Vnc(c.clone()).serialize(serializer),
            SourceConfig::Rdp(c) => SourceConfigInner::Rdp(c.clone()).serialize(serializer),
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
            Protocol::Omt => SourceConfig::Omt(omt::OmtSourceConfig::default()),
            Protocol::Decklink => SourceConfig::Decklink(decklink::DecklinkSourceConfig::default()),
            #[cfg(target_os = "macos")]
            Protocol::Syphon => SourceConfig::Syphon(syphon::SyphonSourceConfig::default()),
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation => SourceConfig::AvFoundation(avfoundation::AvFoundationSourceConfig::default()),
            #[cfg(target_os = "macos")]
            Protocol::ScreenCaptureKit => SourceConfig::ScreenCaptureKit(screencapturekit::ScreenCaptureKitSourceConfig::default()),
            #[cfg(target_os = "windows")]
            Protocol::Spout => SourceConfig::Spout(spout::SpoutSourceConfig::default()),
            #[cfg(target_os = "windows")]
            Protocol::MediaFoundation => SourceConfig::MediaFoundation(mediafoundation::MediaFoundationSourceConfig::default()),
            #[cfg(target_os = "windows")]
            Protocol::DirectShow => SourceConfig::DirectShow(directshow::DirectShowSourceConfig::default()),
            #[cfg(target_os = "windows")]
            Protocol::WindowsCapture => SourceConfig::WindowsCapture(windowscapture::WindowsCaptureSourceConfig::default()),
            Protocol::Vnc => SourceConfig::Vnc(vnc::VncSourceConfig::default()),
            Protocol::Rdp => SourceConfig::Rdp(rdp::RdpSourceConfig::default()),
            Protocol::Unknown(_) => SourceConfig::default(),
        }
    }
}

/// Data a source needs to open that cannot be derived from the `SourceKey`
/// alone. Never persisted — it lives only in the registry so respawn can
/// reopen the source without rediscovery.
#[derive(Clone, Debug)]
pub enum SourceRuntimeConfig {
    Test,
    Ndi {
        discovered: NdiSourceInfo
    },
    Omt {
        discovered: OmtSourceInfo
    },
    Decklink {
        supported_connections: decklink::DecklinkVideoConnections
    },
    #[cfg(target_os = "macos")]
    Syphon {
        info: SyphonServerInfo
    },
    #[cfg(target_os = "macos")]
    AvFoundation {
        modes: Arc<Mutex<Vec<avfoundation::AvFoundationMode>>>,
    },
    #[cfg(target_os = "macos")]
    ScreenCaptureKit {
        label: String
    },
    #[cfg(target_os = "windows")]
    Spout,
    #[cfg(target_os = "windows")]
    MediaFoundation {
        label: String,
        modes: Arc<Mutex<Vec<mediafoundation::MediaFoundationMode>>>,
    },
    #[cfg(target_os = "windows")]
    DirectShow {
        label: String,
        modes: Arc<Mutex<Vec<directshow::DirectShowMode>>>,
    },
    #[cfg(target_os = "windows")]
    WindowsCapture {
        label: String,
    },
    Vnc,
    Rdp,
}

/// A live source: its boxed runtime, the config persisted in the project, and
/// the runtime-only data needed to open it. Protocol-agnostic from the
/// outside — construction is the only place that knows concrete types.
pub struct SourceKind {
    source: Box<dyn VideoSource>,
    pub config: SourceConfig,
    pub runtime: SourceRuntimeConfig,
}

impl SourceKind {
    /// A kind that resolves but has no live source behind it yet. A restart puts
    /// one in place while the old source is released and its replacement opens:
    /// the device has to be free before it is reopened, and every binding on the
    /// key (frames, stats, settings) still has to resolve.
    fn pending(config: SourceConfig, runtime: SourceRuntimeConfig) -> Self {
        return Self {
            source: Box::new(PendingSource {
                stats: Arc::new(Mutex::new(SourceStats::new())),
            }),
            config,
            runtime,
        };
    }

    fn new(key: &SourceKey, config: SourceConfig, runtime: SourceRuntimeConfig) -> Self {
        let source: Box<dyn VideoSource> = match (&config, &runtime) {
            (SourceConfig::Test(c), SourceRuntimeConfig::Test) => {
                Box::new(test::TestSource::spawn(key.source_ref.clone(), c))
            }
            (SourceConfig::Ndi(c), SourceRuntimeConfig::Ndi { discovered }) => {
                Box::new(ndi::NdiSource::spawn(key.source_ref.clone(), discovered.clone(), c))
            }
            (SourceConfig::Omt(c), SourceRuntimeConfig::Omt { discovered }) => {
                Box::new(omt::OmtSource::spawn(key.source_ref.clone(), discovered.clone(), c))
            }
            (SourceConfig::Decklink(c), SourceRuntimeConfig::Decklink { .. }) => {
                Box::new(decklink::DecklinkSource::spawn(key.source_ref.clone(), c))
            }
            #[cfg(target_os = "macos")]
            (SourceConfig::Syphon(_), SourceRuntimeConfig::Syphon { info }) => {
                Box::new(syphon::SyphonSource::spawn(key.source_ref.clone(), info.clone()))
            }
            #[cfg(target_os = "macos")]
            (SourceConfig::AvFoundation(c), SourceRuntimeConfig::AvFoundation { modes }) => {
                Box::new(avfoundation::AvFoundationSource::spawn(key.source_ref.clone(), c, modes.clone()))
            }
            #[cfg(target_os = "macos")]
            (SourceConfig::ScreenCaptureKit(c), SourceRuntimeConfig::ScreenCaptureKit { .. }) => {
                Box::new(screencapturekit::ScreenCaptureKitSource::spawn(key.source_ref.clone(), c))
            }
            #[cfg(target_os = "windows")]
            (SourceConfig::Spout(_), SourceRuntimeConfig::Spout) => {
                Box::new(spout::SpoutSource::spawn(key.source_ref.clone()))
            }
            #[cfg(target_os = "windows")]
            (SourceConfig::MediaFoundation(c), SourceRuntimeConfig::MediaFoundation { modes, .. }) => {
                Box::new(mediafoundation::MediaFoundationSource::spawn(key.source_ref.clone(), c, modes.clone()))
            }
            #[cfg(target_os = "windows")]
            (SourceConfig::DirectShow(c), SourceRuntimeConfig::DirectShow { modes, .. }) => {
                Box::new(directshow::DirectShowSource::spawn(key.source_ref.clone(), c, modes.clone()))
            }
            #[cfg(target_os = "windows")]
            (SourceConfig::WindowsCapture(c), SourceRuntimeConfig::WindowsCapture { .. }) => {
                Box::new(windowscapture::WindowsCaptureSource::spawn(key.source_ref.clone(), c))
            }
            (SourceConfig::Vnc(c), SourceRuntimeConfig::Vnc) => {
                Box::new(vnc::VncSource::spawn(key.source_ref.clone(), c))
            }
            (SourceConfig::Rdp(c), SourceRuntimeConfig::Rdp) => {
                Box::new(rdp::RdpSource::spawn(key.source_ref.clone(), c))
            }
            // Every add_* pairs one protocol's config with its own runtime
            // variant; no other pairing can exist.
            _ => unreachable!("source config paired with a foreign runtime config"),
        };
        return Self { source, config, runtime };
    }

    /// Rebuild this source under the same key from the config and runtime data.
    /// The old runtime is dropped before the new one is constructed.
    fn respawn(self, key: &SourceKey) -> Self {
        let Self { source, config, runtime } = self;
        drop(source);
        return Self::new(key, config, runtime);
    }

    pub fn latest(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Frame> {
        return self.source.latest(device, queue);
    }

    pub fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.source.stats();
    }

    pub fn to_config(&self) -> SourceConfig {
        return self.config.clone();
    }

    /// Human-readable label for source selection. Handles protocol specifics labels
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn display_label(&self, source_ref: &str) -> String {
        #[cfg(target_os = "macos")]
        {
            if let SourceRuntimeConfig::Syphon { info } = &self.runtime {
                return syphon::format_syphon_label(&info.app_name, &info.name);
            }
            if let SourceRuntimeConfig::ScreenCaptureKit { label } = &self.runtime {
                return label.clone();
            }
        }
        #[cfg(target_os = "windows")]
        {
            if let SourceRuntimeConfig::MediaFoundation { label, .. } = &self.runtime {
                return label.clone();
            }
            if let SourceRuntimeConfig::DirectShow { label, .. } = &self.runtime {
                return label.clone();
            }
            if let SourceRuntimeConfig::WindowsCapture { label } = &self.runtime {
                return label.clone();
            }
        }
        return source_ref.to_string();
    }
}

/// How long a restart may stay outstanding before its key is released for another attempt.
const RESTART_TIMEOUT: Duration = Duration::from_secs(20);

pub struct RestartResult {
    pub key: SourceKey,
    pub kind: SourceKind,
}

/// The stand-in a restart leaves behind for the key it is rebuilding.
struct PendingSource {
    stats: Arc<Mutex<SourceStats>>,
}

impl VideoSource for PendingSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        return None;
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }
}

// Registry of all live sources. Owned by the UI thread; sources render on their own threads.
pub struct SourceRegistry {
    sources: HashMap<SourceKey, SourceKind>,
    next_test: u32,
    next_vnc: u32,
    next_rdp: u32,
    pending_restarts: HashMap<SourceKey, Instant>,
    restart_tx: Sender<RestartResult>,
    restart_rx: Receiver<RestartResult>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            sources: HashMap::new(),
            next_test: 0,
            next_vnc: 0,
            next_rdp: 0,
            pending_restarts: HashMap::new(),
            restart_tx: tx,
            restart_rx: rx,
        }
    }

    /// Register a live source under its key. A key that is already live or
    /// mid-restart is left alone, so every placement binding it shares one receiver.
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
        let kind = SourceKind::new(
            &key,
            SourceConfig::Test(config.unwrap_or_default()),
            SourceRuntimeConfig::Test,
        );
        return self.add(key, kind);
    }

    /// Create a dedicated VNC source under a fresh key. The connection details
    /// live in the config, so the key only has to be unique.
    pub fn add_vnc(&mut self, config: vnc::VncSourceConfig) -> SourceKey {
        let key = loop {
            self.next_vnc += 1;
            let key = SourceKey::new(Protocol::Vnc, format!("VNC {}", self.next_vnc));
            if !self.contains(&key) {
                break key;
            }
        };
        return self.add_vnc_at(key, config);
    }

    /// Create a VNC source under a known key, preserving the binding a project
    /// was saved with.
    pub fn add_vnc_at(&mut self, key: SourceKey, config: vnc::VncSourceConfig) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        let kind = SourceKind::new(&key, SourceConfig::Vnc(config), SourceRuntimeConfig::Vnc);
        self.sources.insert(key.clone(), kind);
        return key;
    }

    /// Create a dedicated RDP source under a fresh key. The connection details
    /// live in the config, so the key only has to be unique.
    pub fn add_rdp(&mut self, config: rdp::RdpSourceConfig) -> SourceKey {
        let key = loop {
            self.next_rdp += 1;
            let key = SourceKey::new(Protocol::Rdp, format!("RDP {}", self.next_rdp));
            if !self.contains(&key) {
                break key;
            }
        };
        return self.add_rdp_at(key, config);
    }

    /// Create an RDP source under a known key, preserving the binding a project
    /// was saved with.
    pub fn add_rdp_at(&mut self, key: SourceKey, config: rdp::RdpSourceConfig) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        let kind = SourceKind::new(&key, SourceConfig::Rdp(config), SourceRuntimeConfig::Rdp);
        self.sources.insert(key.clone(), kind);
        return key;
    }

    pub fn add_ndi(&mut self, key: SourceKey, config: ndi::NdiSourceConfig, discovered: NdiSourceInfo) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        let kind = SourceKind::new(
            &key,
            SourceConfig::Ndi(config),
            SourceRuntimeConfig::Ndi { discovered },
        );
        self.sources.insert(key.clone(), kind);
        return key;
    }

    pub fn add_omt(&mut self, key: SourceKey, config: omt::OmtSourceConfig, discovered: OmtSourceInfo) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        let kind = SourceKind::new(
            &key,
            SourceConfig::Omt(config),
            SourceRuntimeConfig::Omt { discovered },
        );
        self.sources.insert(key.clone(), kind);
        return key;
    }

    pub fn add_decklink(&mut self, key: SourceKey, config: decklink::DecklinkSourceConfig, supported_connections: decklink::DecklinkVideoConnections) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        let kind = SourceKind::new(
            &key,
            SourceConfig::Decklink(config),
            SourceRuntimeConfig::Decklink { supported_connections },
        );
        self.sources.insert(key.clone(), kind);
        return key;
    }

    #[cfg(target_os = "macos")]
    pub fn add_syphon(&mut self, key: SourceKey, config: syphon::SyphonSourceConfig, info: SyphonServerInfo) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        let kind = SourceKind::new(
            &key,
            SourceConfig::Syphon(config),
            SourceRuntimeConfig::Syphon { info },
        );
        self.sources.insert(key.clone(), kind);
        return key;
    }

    #[cfg(target_os = "macos")]
    pub fn add_avfoundation(&mut self, key: SourceKey, config: avfoundation::AvFoundationSourceConfig) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        let kind = SourceKind::new(
            &key,
            SourceConfig::AvFoundation(config),
            SourceRuntimeConfig::AvFoundation {
                modes: Arc::new(Mutex::new(Vec::new())),
            },
        );
        self.sources.insert(key.clone(), kind);
        return key;
    }

    #[cfg(target_os = "macos")]
    pub fn add_screencapturekit(&mut self, key: SourceKey, config: screencapturekit::ScreenCaptureKitSourceConfig, label: String) -> SourceKey {
        // First SCK source in the process triggers the one-time TCC prompt.
        screencapturekit::ensure_screen_capture_access_requested();
        if self.contains(&key) {
            return key;
        }
        let kind = SourceKind::new(
            &key,
            SourceConfig::ScreenCaptureKit(config),
            SourceRuntimeConfig::ScreenCaptureKit { label },
        );
        self.sources.insert(key.clone(), kind);
        return key;
    }

    #[cfg(target_os = "windows")]
    pub fn add_spout(&mut self, key: SourceKey, config: spout::SpoutSourceConfig) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        let kind = SourceKind::new(&key, SourceConfig::Spout(config), SourceRuntimeConfig::Spout);
        self.sources.insert(key.clone(), kind);
        return key;
    }

    #[cfg(target_os = "windows")]
    pub fn add_mediafoundation(&mut self, key: SourceKey, config: mediafoundation::MediaFoundationSourceConfig, label: String) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        tracing::debug!(source_ref = %key.source_ref, label = %label, "add_mediafoundation");
        let kind = SourceKind::new(
            &key,
            SourceConfig::MediaFoundation(config),
            SourceRuntimeConfig::MediaFoundation {
                label,
                modes: Arc::new(Mutex::new(Vec::new())),
            },
        );
        self.sources.insert(key.clone(), kind);
        return key;
    }

    #[cfg(target_os = "windows")]
    pub fn add_directshow(&mut self, key: SourceKey, config: directshow::DirectShowSourceConfig, label: String) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        tracing::debug!(source_ref = %key.source_ref, label = %label, "add_directshow");
        let kind = SourceKind::new(
            &key,
            SourceConfig::DirectShow(config),
            SourceRuntimeConfig::DirectShow {
                label,
                modes: Arc::new(Mutex::new(Vec::new())),
            },
        );
        self.sources.insert(key.clone(), kind);
        return key;
    }

    #[cfg(target_os = "windows")]
    pub fn add_windows_capture(&mut self, key: SourceKey, config: windowscapture::WindowsCaptureSourceConfig, label: String) -> SourceKey {
        if self.contains(&key) {
            return key;
        }
        tracing::debug!(source_ref = %key.source_ref, label = %label, "add_windows_capture");
        let kind = SourceKind::new(
            &key,
            SourceConfig::WindowsCapture(config),
            SourceRuntimeConfig::WindowsCapture { label },
        );
        self.sources.insert(key.clone(), kind);
        return key;
    }

    /// Whether a source for this key is live or being restarted.
    pub fn contains(&self, key: &SourceKey) -> bool {
        self.sources.contains_key(key) || self.pending_restarts.contains_key(key)
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

    /// Restart the source under `key` with its current config.
    pub fn restart(&mut self, key: &SourceKey) {
        if self.pending_restarts.contains_key(key) {
            return;
        }
        let Some(live) = self.sources.remove(key) else {
            return;
        };
        // rebuild inline (no blocking work)
        let inline = match &live.runtime {
            SourceRuntimeConfig::Test => true,
            #[cfg(target_os = "windows")]
            SourceRuntimeConfig::Spout => true,
            _ => false,
        };
        if inline {
            self.sources.insert(key.clone(), live.respawn(key));
            return;
        }
        let config = live.config.clone();
        let runtime = live.runtime.clone();
        self.sources
            .insert(key.clone(), SourceKind::pending(config.clone(), runtime.clone()));
        self.pending_restarts.insert(key.clone(), Instant::now());
        let key = key.clone();
        let tx = self.restart_tx.clone();
        let thread_name =
            format!("{}-restart-{}", key.protocol.name().to_lowercase(), key.source_ref);
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                // The old source is released on this thread, so opening the replacement never contends with it
                drop(live);
                let kind = SourceKind::new(&key, config, runtime);
                let _ = tx.send(RestartResult { key, kind });
            })
            .expect("spawn source restart thread");
    }

    pub fn apply_pending_restarts(&mut self) {
        // A teardown that never returns must not make a source unrebuildable:
        // give up on the wait and let the next request try again.
        let now = Instant::now();
        let stuck: Vec<SourceKey> = self
            .pending_restarts
            .iter()
            .filter(|(_, started)| now.duration_since(**started) >= RESTART_TIMEOUT)
            .map(|(key, _)| key.clone())
            .collect();
        for key in stuck {
            self.pending_restarts.remove(&key);
            tracing::warn!(
                source = key.source_ref,
                "Gave up waiting for a source restart to finish; it stays idle until it is restarted"
            );
        }

        while let Ok(mut result) = self.restart_rx.try_recv() {
            self.pending_restarts.remove(&result.key);
            // A config edited while the replacement was being built is newer
            // than the one it was built from: keep it, and build once more
            let newer = match self.sources.get(&result.key) {
                Some(kind) if kind.config != result.kind.config => Some(kind.config.clone()),
                _ => None,
            };
            let edited = newer.is_some();
            if let Some(config) = newer {
                result.kind.config = config;
            }
            // The entry in the map is the pending stand-in, so dropping it inline
            // is instant; the live source was already dropped on the restart thread.
            self.sources.insert(result.key.clone(), result.kind);
            if edited {
                self.restart(&result.key);
            }
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
        self.next_vnc = 0;
        self.next_rdp = 0;
        self.pending_restarts.clear();
        while self.restart_rx.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source_ref alone is not a registry identity: two protocols may carry
    /// the same reference without one lookup hitting the other's source.
    #[test]
    fn keys_are_protocol_scoped() {
        let mut registry = SourceRegistry::new();
        let key = SourceKey::new(Protocol::Test, "Camera".into());
        registry.add(key.clone(), SourceKind::new(
            &key,
            SourceConfig::Test(Default::default()),
            SourceRuntimeConfig::Test,
        ));

        assert!(registry.contains(&key));
        let other_protocol = SourceKey::new(Protocol::Ndi, "Camera".into());
        assert!(!registry.contains(&other_protocol));
        assert!(registry.get(&other_protocol).is_none());
        assert_eq!(registry.list_sources(Protocol::Test).len(), 1);
        assert_eq!(registry.list_sources(Protocol::Ndi).len(), 0);
    }

    /// A second placement binding the same protocol + source_ref shares the one
    /// live receiver instead of spawning another.
    #[test]
    fn duplicate_key_shares_one_runtime_source() {
        let mut registry = SourceRegistry::new();
        let key = registry.add_test(None);
        let count = registry.iter().count();

        let again = registry.add(key.clone(), SourceKind::new(
            &key,
            SourceConfig::Test(Default::default()),
            SourceRuntimeConfig::Test,
        ));
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

    /// VNC sources are keyed by a synthesized name; several must not collide.
    #[test]
    fn add_vnc_keys_stay_unique() {
        let mut registry = SourceRegistry::new();
        let mut keys = HashSet::new();
        for _ in 0..5 {
            let key = registry.add_vnc(vnc::VncSourceConfig::default());
            assert!(keys.insert(key.clone()), "duplicate key {:?}", key);
        }
        assert_eq!(registry.iter().count(), 5);
    }

    /// A synthesized key never overwrites a source restored from a project.
    #[test]
    fn add_vnc_skips_a_restored_key() {
        let mut registry = SourceRegistry::new();
        let restored = SourceKey::new(Protocol::Vnc, "VNC 1".to_string());
        registry.add_vnc_at(restored, vnc::VncSourceConfig::default());
        let key = registry.add_vnc(vnc::VncSourceConfig::default());
        assert_eq!(key.source_ref, "VNC 2");
    }

    /// A restart rebuilds the source under the same key.
    #[test]
    fn restart_keeps_the_key() {
        let mut registry = SourceRegistry::new();
        let key = registry.add_test(None);
        registry.restart(&key);
        assert!(registry.get(&key).is_some());
    }

    /// A source being rebuilt on a thread stays live and keeps its config, so
    /// everything keyed on it (frames, stats, config UI) still resolves while
    /// the replacement is built.
    #[test]
    fn a_threaded_restart_keeps_the_source_resolvable() {
        let mut registry = SourceRegistry::new();
        let key = registry.add_vnc(vnc::VncSourceConfig {
            host: "studio.example".to_string(),
            ..Default::default()
        });
        registry.restart(&key);

        let live = registry.get(&key).expect("source vanished during its restart");
        assert!(matches!(&live.config, SourceConfig::Vnc(config) if config.host == "studio.example"));

        // The replacement lands under the same key, and asking again while one
        // is in flight does not queue a second build.
        registry.restart(&key);
        let deadline = Instant::now() + Duration::from_secs(5);
        while registry.pending_restarts.contains_key(&key) && Instant::now() < deadline {
            registry.apply_pending_restarts();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!registry.pending_restarts.contains_key(&key));
        assert!(registry.get(&key).is_some());
    }

    /// A config edited while a replacement was being built is newer than the one
    /// that replacement was built from: it is kept, and the source is built once
    /// more so nothing the user typed is silently reverted.
    #[test]
    fn an_edit_during_a_restart_survives_the_swap() {
        let mut registry = SourceRegistry::new();
        let first = vnc::VncSourceConfig {
            host: "one.example".to_string(),
            ..Default::default()
        };
        let edited = SourceConfig::Vnc(vnc::VncSourceConfig {
            host: "two.example".to_string(),
            ..first.clone()
        });
        let key = registry.add_vnc(first.clone());

        // The user's newer edit is already on the live source when the
        // replacement, built from the older config, lands.
        registry.sources.get_mut(&key).unwrap().config = edited.clone();
        registry.pending_restarts.insert(key.clone(), Instant::now());
        let stale = SourceKind::new(&key, SourceConfig::Vnc(first), SourceRuntimeConfig::Vnc);
        registry.restart_tx.send(RestartResult { key: key.clone(), kind: stale }).unwrap();

        registry.apply_pending_restarts();

        assert_eq!(registry.get(&key).map(|kind| kind.config.clone()), Some(edited));
    }

    /// A teardown that never finishes must not strand the key: after the wait it
    /// is released so the source can be restarted again, and it stays resolvable
    /// so its settings remain reachable.
    #[test]
    fn a_stuck_restart_is_given_up_on() {
        let mut registry = SourceRegistry::new();
        let key = registry.add_vnc(vnc::VncSourceConfig {
            host: "studio.example".to_string(),
            ..Default::default()
        });
        registry.pending_restarts.insert(
            key.clone(),
            Instant::now() - RESTART_TIMEOUT - Duration::from_secs(1),
        );

        registry.apply_pending_restarts();

        assert!(!registry.pending_restarts.contains_key(&key));
        assert!(registry.get(&key).is_some());

        registry.restart(&key);
        assert!(registry.pending_restarts.contains_key(&key));
    }
}
