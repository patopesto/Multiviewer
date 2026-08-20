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
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(tag = "protocol")]
pub enum OutputConfig {
    #[cfg(target_os = "macos")]
    Syphon(syphon::SyphonOutputConfig),
    Ndi(ndi::NdiOutputConfig),
    Decklink(decklink::DecklinkOutputConfig),
    #[default]
    #[serde(other)]
    Unknown,
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
        let output = syphon::SyphonOutput::new(id.clone(), name, config, enabled);
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
}
