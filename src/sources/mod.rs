use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub mod decklink;
pub mod ndi;
pub mod test;

#[cfg(target_os = "macos")]
pub mod syphon;

pub use decklink::source::{DecklinkConfig, DecklinkSource};
pub use ndi::source::{NdiConfig, NdiSource};
pub use test::{TestConfig, TestSource};

#[cfg(target_os = "macos")]
pub use syphon::source::{SyphonConfig, SyphonSource};

pub type SourceId = String;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    Bgra8,
}

#[derive(Clone)]
pub struct CpuFrame {
    pub data: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
    pub fmt: PixelFormat,
    /// Monotonic per-source counter; compositor uploads only when this changes.
    pub seq: u64,
}

#[derive(Clone)]
pub struct SyphonFrame {
    pub bg: Arc<wgpu::BindGroup>,
    pub w: u32,
    pub h: u32,
    #[allow(dead_code)]
    pub seq: u64,
}

#[derive(Clone)]
pub enum Frame {
    Cpu(CpuFrame),
    Syphon(SyphonFrame),
}

pub trait VideoSource: Send + Sync {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame>;
    #[allow(dead_code)]
    fn name(&self) -> &str;
}

pub enum SourceKind {
    Test(TestSource, TestConfig),
    Ndi(NdiSource, NdiConfig, grafton_ndi::Source),
    #[cfg(target_os = "macos")]
    Syphon(SyphonSource, SyphonConfig, String),
    Decklink(DecklinkSource, DecklinkConfig, String),
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

    pub fn is_test(&self) -> bool {
        matches!(self, SourceKind::Test(_, _))
    }

    pub fn is_syphon(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            matches!(self, SourceKind::Syphon(_, _, _))
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn is_decklink(&self) -> bool {
        matches!(self, SourceKind::Decklink(_, _, _))
    }

    #[allow(dead_code)]
    pub fn test_config_mut(&mut self) -> Option<&mut TestConfig> {
        match self {
            SourceKind::Test(_, cfg) => Some(cfg),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn ndi_config_mut(&mut self) -> Option<&mut NdiConfig> {
        match self {
            SourceKind::Ndi(_, cfg, _) => Some(cfg),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn syphon_config_mut(&mut self) -> Option<&mut SyphonConfig> {
        #[cfg(target_os = "macos")]
        {
            match self {
                SourceKind::Syphon(_, cfg, _) => Some(cfg),
                _ => None,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    #[allow(dead_code)]
    pub fn decklink_config_mut(&mut self) -> Option<&mut DecklinkConfig> {
        match self {
            SourceKind::Decklink(_, cfg, _) => Some(cfg),
            _ => None,
        }
    }
}

/// All live sources. Owned by the UI thread; sources render on their own threads.
pub struct Registry {
    sources: HashMap<SourceId, SourceKind>,
    next_test: u32,
}

impl Registry {
    pub fn new() -> Self {
        Self { sources: HashMap::new(), next_test: 0 }
    }

    /// Create a dedicated test source for a layer.
    pub fn add_test(&mut self) -> SourceId {
        self.next_test += 1;
        let letter = (b'A' + (self.next_test as u8 - 1) % 26) as char;
        let id = format!("Test {letter}");
        let src = TestSource::spawn(id.clone(), self.next_test);
        self.sources.insert(id.clone(), SourceKind::Test(src, TestConfig::default()));
        id
    }

    pub fn add_ndi(&mut self, name: String, source: grafton_ndi::Source) -> SourceId {
        if self.sources.contains_key(&name) {
            return name;
        }
        let cfg = NdiConfig::default();
        let src = NdiSource::spawn(name.clone(), source.clone(), &cfg);
        self.sources.insert(name.clone(), SourceKind::Ndi(src, cfg, source));
        name
    }

    #[cfg(target_os = "macos")]
    pub fn add_syphon(&mut self, name: String, server_name: String) -> SourceId {
        if self.sources.contains_key(&name) {
            return name;
        }
        let src = SyphonSource::spawn(name.clone(), server_name);
        self.sources.insert(name.clone(), SourceKind::Syphon(src, SyphonConfig::default(), name.clone()));
        name
    }

    pub fn add_decklink(&mut self, name: String, display_name: String, supported_connections: Option<String>) -> SourceId {
        if self.sources.contains_key(&name) {
            return name;
        }
        let mut cfg = DecklinkConfig::default();
        if let Some(conn) = supported_connections {
            cfg.supported_connections = conn;
        }
        let src = DecklinkSource::spawn(name.clone(), display_name, &cfg);
        self.sources.insert(name.clone(), SourceKind::Decklink(src, cfg, name.clone()));
        name
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

    /// Restart an NDI source with its current config.
    pub fn restart_ndi(&mut self, name: &str) {
        if let Some(SourceKind::Ndi(_, cfg, source)) = self.sources.remove(name) {
            let new = NdiSource::spawn(name.to_string(), source.clone(), &cfg);
            self.sources.insert(name.to_string(), SourceKind::Ndi(new, cfg, source));
        }
    }

    #[cfg(target_os = "macos")]
    pub fn restart_syphon(&mut self, name: &str) {
        if let Some(SourceKind::Syphon(_, cfg, server_name)) = self.sources.remove(name) {
            let new = SyphonSource::spawn(name.to_string(), server_name);
            self.sources.insert(name.to_string(), SourceKind::Syphon(new, cfg, name.to_string()));
        }
    }

    pub fn restart_decklink(&mut self, name: &str) {
        if let Some(SourceKind::Decklink(_, cfg, display_name)) = self.sources.remove(name) {
            let new = DecklinkSource::spawn(name.to_string(), display_name, &cfg);
            self.sources.insert(name.to_string(), SourceKind::Decklink(new, cfg, name.to_string()));
        }
    }

    /// Remove all sources not referenced by any layer.
    pub fn cleanup_orphaned_sources(&mut self, active_source_ids: &[&str]) {
        let active: HashSet<&str> = active_source_ids.iter().copied().collect();
        let to_remove: Vec<String> = self.sources
            .iter()
            .filter(|(id, _)| !active.contains(id.as_str()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in to_remove {
            self.sources.remove(&id);
        }
    }

    pub fn list_test_sources(&self) -> Vec<(&SourceId, &SourceKind)> {
        self.sources.iter().filter(|(_, sk)| sk.is_test()).collect()
    }

    pub fn list_ndi_sources(&self) -> Vec<(&SourceId, &SourceKind)> {
        self.sources.iter().filter(|(_, sk)| !sk.is_test() && !sk.is_syphon() && !sk.is_decklink()).collect()
    }

    #[cfg(target_os = "macos")]
    pub fn list_syphon_sources(&self) -> Vec<(&SourceId, &SourceKind)> {
        self.sources.iter().filter(|(_, sk)| sk.is_syphon()).collect()
    }

    pub fn list_decklink_sources(&self) -> Vec<(&SourceId, &SourceKind)> {
        self.sources.iter().filter(|(_, sk)| sk.is_decklink()).collect()
    }
}
