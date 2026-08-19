use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::Protocol;
use super::decklink::{DecklinkOutput, DecklinkOutputConfig};
use super::ndi::{NdiOutput, NdiOutputConfig};
#[cfg(target_os = "macos")]
use super::syphon::{SyphonOutput, SyphonOutputConfig};


pub type OutputId = String;

#[derive(Debug, Clone, Default)]
pub struct OutputStats {
    pub width: u32,
    pub height: u32,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub send_time_ms: f32,
}

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

/// Type-erased output handle stored in the registry.
pub type OutputKind = Box<dyn VideoOutput>;

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
        config: NdiOutputConfig,
        enabled: bool,
    ) -> OutputId {
        if self.outputs.contains_key(&id) {
            return id;
        }
        let output = NdiOutput::new(id.clone(), name, config, enabled);
        self.outputs.insert(id.clone(), Box::new(output));
        id
    }

    pub fn add_decklink(
        &mut self,
        id: OutputId,
        name: String,
        config: DecklinkOutputConfig,
        enabled: bool,
    ) -> OutputId {
        if self.outputs.contains_key(&id) {
            return id;
        }
        let output = DecklinkOutput::new(id.clone(), name, config, enabled);
        self.outputs.insert(id.clone(), Box::new(output));
        id
    }

    #[cfg(target_os = "macos")]
    pub fn add_syphon(&mut self, id: OutputId, name: String, config: SyphonOutputConfig, enabled: bool) -> OutputId {
        if self.outputs.contains_key(&id) {
            return id;
        }
        let output = SyphonOutput::new(id.clone(), name, config, enabled);
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
