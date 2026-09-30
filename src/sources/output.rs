use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use serde::{Serialize, Deserialize};

use super::Protocol;
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
    );
    fn name(&self) -> &str;
    fn enabled(&self) -> bool;
    fn set_enabled(&self, enabled: bool);
    fn stats(&self) -> Arc<Mutex<OutputStats>>;
    fn protocol(&self) -> Protocol;
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

    pub fn present(&self, texture: &wgpu::Texture, width: u32, height: u32, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.output.present(texture, width, height, device, queue);
    }

    pub fn enabled(&self) -> bool {
        return self.output.enabled();
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.output.set_enabled(enabled);
    }
}

#[derive(Debug, Clone, Default)]
pub struct OutputStats {
    pub width: u32,
    pub height: u32,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub send_time_ms: f32,
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

    pub fn any_enabled(&self) -> bool {
        self.outputs.iter().any(|(_, ok)| ok.enabled())
    }

    pub fn present_all(&self, texture: &wgpu::Texture, width: u32, height: u32, device: &wgpu::Device, queue: &wgpu::Queue) {
        for output in self.outputs.values() {
            if output.enabled() {
                output.present(texture, width, height, device, queue);
            }
        }
    }

    pub fn clear(&mut self) {
        self.outputs.clear();
    }
}
