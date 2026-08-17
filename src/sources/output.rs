use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub type OutputId = String;

pub trait VideoOutput: Send + Sync {
    fn present(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    );
    #[allow(dead_code)]
    fn name(&self) -> &str;
    fn enabled(&self) -> &bool;
    fn stats(&self) -> Arc<Mutex<OutputStats>>;
}

#[derive(Serialize, Deserialize, Clone)]
pub enum OutputKind {
    Ndi(NdiOutputConfig),
    Syphon(SyphonOutputConfig),
    Decklink(DecklinkOutputConfig),
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct NdiOutputConfig {
    pub sender_name: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SyphonOutputConfig {
    pub server_name: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct DecklinkOutputConfig {
    pub display_name: String,
    pub display_mode: u32,
    pub pixel_format: u32,
}

#[derive(Debug, Clone)]
pub struct OutputStats {
    pub width: u32,
    pub height: u32,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub send_time_ms: f32,
}

impl Default for OutputStats {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            frames_sent: 0,
            frames_dropped: 0,
            send_time_ms: 0.0,
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
        false
        // self.outputs.iter().any(|(_, ok)| ok.enabled())
    }
}
