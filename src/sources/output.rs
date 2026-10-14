use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};
use tracing::instrument;

use super::{OutputStats, Protocol};
use super::decklink;
use super::ndi;
#[cfg(target_os = "macos")]
use super::syphon;
#[cfg(target_os = "windows")]
use super::spout;


pub type OutputId = String;

/// Runtime video output. Implemented by NDI, Syphon, and DeckLink.
#[allow(dead_code)]
pub trait VideoOutput: Send {
    fn present(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool;
    fn finish(&self) {}
    fn name(&self) -> &str;
    fn enabled(&self) -> bool;
    fn set_enabled(&self, enabled: bool);
    fn stats(&self) -> Arc<Mutex<OutputStats>>;
    fn protocol(&self) -> Protocol;
    fn busy(&self) -> bool { // overriden by VideoOutputs. true if consumer hasn't taken previous frame
        return false;
    }
}

/// Protocol-specific output configuration stored in the config file.
/// New protocols (NDI, DeckLink, ...) add variants here.
///
/// `Unknown` doubles as the "no config" default (`Value::Null`) and as the
/// carrier for config JSON this platform cannot interpret: the original value
/// is kept untouched and written back on save, so a project round-trips to a
/// platform that does understand it.
#[derive(Clone)]
pub enum OutputConfig {
    Ndi(ndi::NdiOutputConfig),
    Decklink(decklink::DecklinkOutputConfig),
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonOutputConfig),
    #[cfg(target_os = "windows")]
    Spout(spout::SpoutOutputConfig),
    Unknown(serde_json::Value),
}

impl Default for OutputConfig {
    fn default() -> Self {
        OutputConfig::Unknown(serde_json::Value::Null)
    }
}

impl OutputConfig {
    /// Whether this config belongs to `protocol`.
    fn matches(&self, protocol: &Protocol) -> bool {
        return match (self, protocol) {
            (OutputConfig::Ndi(_), Protocol::Ndi) => true,
            (OutputConfig::Decklink(_), Protocol::Decklink) => true,
            #[cfg(target_os = "macos")]
            (OutputConfig::Syphon(_), Protocol::Syphon) => true,
            #[cfg(target_os = "windows")]
            (OutputConfig::Spout(_), Protocol::Spout) => true,
            _ => false,
        };
    }

    /// Default config for a protocol. Protocols without an output (Test,
    /// AVFoundation, unavailable ones) fall back to the unknown default.
    fn for_protocol(protocol: &Protocol) -> Self {
        return match protocol {
            Protocol::Ndi => OutputConfig::Ndi(ndi::NdiOutputConfig::default()),
            Protocol::Decklink => OutputConfig::Decklink(decklink::DecklinkOutputConfig::default()),
            #[cfg(target_os = "macos")]
            Protocol::Syphon => OutputConfig::Syphon(syphon::SyphonOutputConfig::default()),
            #[cfg(target_os = "windows")]
            Protocol::Spout => OutputConfig::Spout(spout::SpoutOutputConfig::default()),
            _ => OutputConfig::default(),
        };
    }
}

/// Known variants in their original wire format (internally tagged with
/// `protocol`); used by both directions of the custom serde impls below.
#[derive(Serialize, Deserialize)]
#[serde(tag = "protocol")]
enum OutputConfigInner {
    Ndi(ndi::NdiOutputConfig),
    Decklink(decklink::DecklinkOutputConfig),
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonOutputConfig),
    #[cfg(target_os = "windows")]
    Spout(spout::SpoutOutputConfig),
}

impl From<OutputConfigInner> for OutputConfig {
    fn from(inner: OutputConfigInner) -> Self {
        match inner {
            OutputConfigInner::Ndi(c) => OutputConfig::Ndi(c),
            OutputConfigInner::Decklink(c) => OutputConfig::Decklink(c),
            #[cfg(target_os = "macos")]
            OutputConfigInner::Syphon(c) => OutputConfig::Syphon(c),
            #[cfg(target_os = "windows")]
            OutputConfigInner::Spout(c) => OutputConfig::Spout(c),
        }
    }
}

impl Serialize for OutputConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            OutputConfig::Unknown(v) if v.is_null() => {
                // Historical wire form of a missing/default config.
                serde_json::json!({"protocol": "Unknown"}).serialize(serializer)
            }
            OutputConfig::Unknown(v) => v.serialize(serializer),
            OutputConfig::Ndi(c) => OutputConfigInner::Ndi(c.clone()).serialize(serializer),
            OutputConfig::Decklink(c) => OutputConfigInner::Decklink(c.clone()).serialize(serializer),
            #[cfg(target_os = "macos")]
            OutputConfig::Syphon(c) => OutputConfigInner::Syphon(c.clone()).serialize(serializer),
            #[cfg(target_os = "windows")]
            OutputConfig::Spout(c) => OutputConfigInner::Spout(c.clone()).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for OutputConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_null() {
            return Ok(OutputConfig::default());
        }
        match serde_json::from_value::<OutputConfigInner>(value.clone()) {
            Ok(inner) => return Ok(inner.into()),
            // Unrecognized `protocol` tag (a protocol this platform does not
            // have, or a corrupt known one): keep the original JSON verbatim.
            Err(_) => return Ok(OutputConfig::Unknown(value)),
        }
    }
}

/// A live output. Protocol-agnostic from the outside — construction is the
/// only place that knows concrete types.
pub struct OutputKind {
    output: Box<dyn VideoOutput>,
}

impl OutputKind {
    fn new(protocol: &Protocol, id: OutputId, name: String, config: &OutputConfig, enabled: bool) -> Option<Self> {
        // A config written for another protocol (or missing entirely, e.g. a
        // project file from before outputs had configs) falls back to the
        // protocol default so the output still opens.
        let config = if config.matches(protocol) {
            config.clone()
        } else {
            OutputConfig::for_protocol(protocol)
        };
        let output: Box<dyn VideoOutput> = match &config {
            OutputConfig::Ndi(c) => {
                let name = if c.sender_name.is_empty() { name } else { c.sender_name.clone() };
                Box::new(ndi::NdiOutput::new(id, name, c.clone(), enabled))
            }
            OutputConfig::Decklink(c) => {
                let name = if c.device_name.is_empty() { name } else { c.device_name.clone() };
                Box::new(decklink::DecklinkOutput::new(id, name, c.clone(), enabled))
            }
            #[cfg(target_os = "macos")]
            OutputConfig::Syphon(c) => {
                let name = if c.server_name.is_empty() { name } else { c.server_name.clone() };
                Box::new(syphon::SyphonOutput::new(id, name, c.clone(), enabled))
            }
            #[cfg(target_os = "windows")]
            OutputConfig::Spout(c) => {
                let name = if c.sender_name.is_empty() { name } else { c.sender_name.clone() };
                Box::new(spout::SpoutOutput::new(id, name, c.clone(), enabled))
            }
            // Protocol unavailable on this platform: no runtime object.
            OutputConfig::Unknown(_) => return None,
        };
        return Some(Self { output });
    }

    #[instrument(level = "debug", skip_all, fields(protocol = ?self.output.protocol(), output = %self.output.name()))]
    pub fn present(&self, texture: &wgpu::Texture, width: u32, height: u32, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        return self.output.present(texture, width, height, device, queue);
    }

    pub fn finish(&self) {
        self.output.finish();
    }

    pub fn enabled(&self) -> bool {
        return self.output.enabled();
    }

    pub fn busy(&self) -> bool {
        return self.output.busy();
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.output.set_enabled(enabled);
    }

    pub fn stats(&self) -> Arc<Mutex<OutputStats>> {
        return self.output.stats();
    }
}

pub struct OutputRegistry {
    outputs: HashMap<OutputId, OutputKind>,
}

impl OutputRegistry {
    pub fn new() -> Self {
        Self {
            outputs: HashMap::new(),
        }
    }

    /// Register the runtime output for `id`, unless one is already live.
    pub fn add(&mut self, protocol: &Protocol, id: OutputId, name: String, config: &OutputConfig, enabled: bool) -> OutputId {
        if !self.outputs.contains_key(&id) {
            self.insert(protocol, id.clone(), name, config, enabled);
        }
        return id;
    }

    /// Replace the runtime output for `id` with one built from `config`
    pub fn restart(&mut self, protocol: &Protocol, id: OutputId, name: String, config: &OutputConfig, enabled: bool) {
        self.insert(protocol, id, name, config, enabled);
    }

    fn insert(&mut self, protocol: &Protocol, id: OutputId, name: String, config: &OutputConfig, enabled: bool) {
        // Drop any previous runtime first; if the config cannot be built
        // (protocol unavailable on this platform) nothing is reinserted.
        self.outputs.remove(&id);
        if let Some(kind) = OutputKind::new(protocol, id.clone(), name, config, enabled) {
            self.outputs.insert(id, kind);
        }
    }

    pub fn get(&self, id: &OutputId) -> Option<&OutputKind> {
        self.outputs.get(id)
    }

    pub fn get_mut(&mut self, id: &OutputId) -> Option<&mut OutputKind> {
        self.outputs.get_mut(id)
    }

    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = (&OutputId, &OutputKind)> {
        self.outputs.iter()
    }

    #[allow(dead_code)]
    pub fn remove(&mut self, id: &OutputId) {
        self.outputs.remove(id);
    }

    pub fn any_ready(&self) -> bool {
        return self.outputs.iter().any(|(_, ok)| ok.enabled() && !ok.busy());
    }

    pub fn any_enabled(&self) -> bool {
        return self.outputs.values().any(|ok| ok.enabled());
    }

    #[instrument(level = "debug", skip_all)]
    pub fn present_all(&self, texture: &wgpu::Texture, width: u32, height: u32, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut awaiting = false;
        for output in self.outputs.values() {
            if output.enabled() && output.present(texture, width, height, device, queue) {
                awaiting = true;
            }
        }
        if awaiting {
            let wait_span = tracing::debug_span!("readback_wait");
            let _wait_guard = wait_span.entered();
            if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
                tracing::error!("output device poll failed");
            }
        }
        for output in self.outputs.values() {
            output.finish();
        }
    }

    pub fn clear(&mut self) {
        self.outputs.clear();
    }
}


/// How long a frame may sit unconsumed before the worker is treated as stuck
const STALL_TIMEOUT: Duration = Duration::from_secs(2);

/// Per-output worker liveness and stall detection.
#[derive(Default)]
pub struct OutputLiveness {
    exited: Arc<AtomicBool>,
    stall_since: Mutex<Option<Instant>>,
    stall_logged: AtomicBool,
}

impl OutputLiveness {
    /// Move into the worker thread: marks the worker exited when it drops.
    pub fn exit_flag(&self) -> WorkerExit {
        return WorkerExit(Arc::clone(&self.exited));
    }

    pub fn is_alive(&self) -> bool {
        return !self.exited.load(Ordering::Acquire);
    }

    /// Record that a frame was handed to the worker.
    pub fn mark_sent(&self) {
        self.stall_since.lock().unwrap().replace(Instant::now());
        self.stall_logged.store(false, Ordering::Release);
    }

    /// The handoff failed, so nothing is in flight.
    pub fn clear_sent(&self) {
        self.stall_since.lock().unwrap().take();
    }

    /// Log once if a frame has been in flight past `STALL_TIMEOUT`.
    pub fn check_stall(&self, name: &str) {
        if self.stall_logged.load(Ordering::Acquire) {
            return;
        }
        let Some(since) = *self.stall_since.lock().unwrap() else {
            return;
        };
        if since.elapsed() < STALL_TIMEOUT {
            return;
        }
        self.stall_logged.store(true, Ordering::Release);
        tracing::warn!(output = %name, "output stalled: worker has not consumed a frame for {:.1}s", since.elapsed().as_secs_f32());
    }
}

/// Marks the owning output's worker as exited when it drops.
pub struct WorkerExit(Arc<AtomicBool>);

impl Drop for WorkerExit {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
