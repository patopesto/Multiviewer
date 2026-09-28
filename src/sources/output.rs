use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use serde::{Serialize, Deserialize};

use super::Protocol;
use super::decklink;
use super::ndi;
#[cfg(target_os = "macos")]
use super::syphon;


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
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonOutputConfig),
    Ndi(ndi::NdiOutputConfig),
    Decklink(decklink::DecklinkOutputConfig),
    Unknown(serde_json::Value),
}

impl Default for OutputConfig {
    fn default() -> Self {
        OutputConfig::Unknown(serde_json::Value::Null)
    }
}

/// Known variants in their original wire format (internally tagged with
/// `protocol`); used by both directions of the custom serde impls below.
#[derive(Serialize, Deserialize)]
#[serde(tag = "protocol")]
enum OutputConfigInner {
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonOutputConfig),
    Ndi(ndi::NdiOutputConfig),
    Decklink(decklink::DecklinkOutputConfig),
}

impl From<OutputConfigInner> for OutputConfig {
    fn from(inner: OutputConfigInner) -> Self {
        match inner {
            #[cfg(target_os = "macos")]
            OutputConfigInner::Syphon(c) => OutputConfig::Syphon(c),
            OutputConfigInner::Ndi(c) => OutputConfig::Ndi(c),
            OutputConfigInner::Decklink(c) => OutputConfig::Decklink(c),
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
            #[cfg(target_os = "macos")]
            OutputConfig::Syphon(c) => OutputConfigInner::Syphon(c.clone()).serialize(serializer),
            OutputConfig::Ndi(c) => OutputConfigInner::Ndi(c.clone()).serialize(serializer),
            OutputConfig::Decklink(c) => OutputConfigInner::Decklink(c.clone()).serialize(serializer),
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

/// Type-erased output handle stored in the registry.
pub type OutputKind = Box<dyn VideoOutput>;

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

    pub fn add_ndi(
        &mut self,
        id: OutputId,
        name: String,
        config: ndi::NdiOutputConfig,
        enabled: bool,
    ) -> OutputId {
        if self.outputs.contains_key(&id) {
            return id;
        }
        let output = ndi::NdiOutput::new(id.clone(), name, config, enabled);
        self.outputs.insert(id.clone(), Box::new(output));
        id
    }

    pub fn add_decklink(
        &mut self,
        id: OutputId,
        name: String,
        config: decklink::DecklinkOutputConfig,
        enabled: bool,
    ) -> OutputId {
        if self.outputs.contains_key(&id) {
            return id;
        }
        let output = decklink::DecklinkOutput::new(id.clone(), name, config, enabled);
        self.outputs.insert(id.clone(), Box::new(output));
        id
    }

    #[cfg(target_os = "macos")]
    pub fn add_syphon(&mut self, id: OutputId, name: String, config: syphon::SyphonOutputConfig, enabled: bool) -> OutputId {
        if self.outputs.contains_key(&id) {
            return id;
        }
        let output = syphon::SyphonOutput::new(name, config, enabled);
        self.outputs.insert(id.clone(), Box::new(output));
        id
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

    pub fn present_all(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
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
