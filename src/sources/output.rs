use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::Protocol;
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

#[allow(dead_code)]
pub enum OutputKind {
    Ndi(NdiOutput, NdiOutputConfig),
    #[cfg(target_os = "macos")]
    Syphon(SyphonOutput, SyphonOutputConfig),
}

impl OutputKind {
    pub fn present(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        match self {
            OutputKind::Ndi(s, _) => s.present(texture, device, queue),
            #[cfg(target_os = "macos")]
            OutputKind::Syphon(s, _) => s.present(texture, width, height, device, queue),
        }
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        match self {
            OutputKind::Ndi(s, _) => s.name(),
            #[cfg(target_os = "macos")]
            OutputKind::Syphon(s, _) => s.name(),
        }
    }

    pub fn enabled(&self) -> bool {
        match self {
            OutputKind::Ndi(s, _) => s.enabled(),
            #[cfg(target_os = "macos")]
            OutputKind::Syphon(s, _) => s.enabled(),
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        match self {
            OutputKind::Ndi(s, _) => s.set_enabled(enabled),
            #[cfg(target_os = "macos")]
            OutputKind::Syphon(s, _) => s.set_enabled(enabled),
        }
    }

    #[allow(dead_code)]
    pub fn stats(&self) -> Arc<Mutex<OutputStats>> {
        match self {
            OutputKind::Ndi(s, _) => s.stats(),
            #[cfg(target_os = "macos")]
            OutputKind::Syphon(s, _) => s.stats(),
        }
    }

    #[allow(dead_code)]
    pub fn protocol(&self) -> Protocol {
        match self {
            OutputKind::Ndi(_, _) => Protocol::Ndi,
            #[cfg(target_os = "macos")]
            OutputKind::Syphon(_, _) => Protocol::Syphon,
        }
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
        let output = NdiOutput::new(id.clone(), name, config.clone(), enabled);
        self.outputs.insert(id.clone(), OutputKind::Ndi(output, config));
        id
    }

    #[cfg(target_os = "macos")]
    pub fn add_syphon(&mut self, id: OutputId, name: String, config: SyphonOutputConfig, enabled: bool) -> OutputId {
        if self.outputs.contains_key(&id) {
            return id;
        }
        let output = SyphonOutput::new(id.clone(), name, config.clone(), enabled);
        self.outputs.insert(id.clone(), OutputKind::Syphon(output, config));
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
        for (_, output) in &self.outputs {
            if output.enabled() {
                output.present(texture, width, height, device, queue);
            }
        }
    }
}
