use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub mod ndi;
pub mod test;

pub use ndi::source::{NdiConfig, NdiSource};
pub use test::{TestConfig, TestSource};

pub type SourceId = String;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
}

#[derive(Clone)]
pub struct CpuFrame {
    pub data: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
    #[allow(dead_code)]
    pub fmt: PixelFormat,
    /// Monotonic per-source counter; compositor uploads only when this changes.
    pub seq: u64,
}

#[derive(Clone)]
pub enum Frame {
    Cpu(CpuFrame),
}

pub trait VideoSource: Send + Sync {
    fn latest(&self) -> Option<Frame>;
    #[allow(dead_code)]
    fn name(&self) -> &str;
}

pub enum SourceKind {
    Test(TestSource, TestConfig),
    Ndi(NdiSource, NdiConfig, grafton_ndi::Source),
}

impl SourceKind {
    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        match self {
            SourceKind::Test(s, _) => s.name(),
            SourceKind::Ndi(s, _, _) => s.name(),
        }
    }

    pub fn latest(&self) -> Option<Frame> {
        match self {
            SourceKind::Test(s, _) => s.latest(),
            SourceKind::Ndi(s, _, _) => s.latest(),
        }
    }

    pub fn is_test(&self) -> bool {
        matches!(self, SourceKind::Test(_, _))
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
        self.sources.iter().filter(|(_, sk)| !sk.is_test()).collect()
    }
}
