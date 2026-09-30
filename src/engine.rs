use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::compositor::{self, Compositor, Draw, Rect};
use crate::config::{Config, LayerId, Source, TextureMode};
use crate::sources::{Protocol, SourceConfig, SourceKey, OutputConfig, SourceRegistry, OutputRegistry};
use crate::sources::decklink::Discovery as DecklinkDiscovery;
use crate::sources::decklink::{DecklinkSourceConfig, DecklinkOutputConfig};
use crate::sources::ndi::Discovery as NdiDiscovery;
use crate::sources::ndi::{NdiSourceConfig, NdiOutputConfig};
#[cfg(target_os = "macos")]
use crate::sources::syphon::Discovery as SyphonDiscovery;
#[cfg(target_os = "macos")]
use crate::sources::syphon::{SyphonSourceConfig, SyphonOutputConfig};
#[cfg(target_os = "macos")]
use crate::sources::avfoundation::Discovery as AvFoundationDiscovery;
#[cfg(target_os = "macos")]
use crate::sources::avfoundation::AvFoundationSourceConfig;
#[cfg(target_os = "windows")]
use crate::sources::spout::Discovery as SpoutDiscovery;
#[cfg(target_os = "windows")]
use crate::sources::spout::{SpoutSourceConfig, SpoutOutputConfig};

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 20.0;
pub const SNAP_THRESHOLD: f32 = 2.0;
pub const SNAP_BREAK_THRESHOLD: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct ViewState {
    pub zoom: f32,
    pub pan: egui::Vec2,
}

impl ViewState {
    pub fn new() -> Self {
        Self {
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

#[derive(Clone, Copy, Debug)]
pub struct WorldRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, Default)]
pub enum DragState {
    #[default]
    None,
    Move {
        uuid: String,
    },
    Resize {
        uuid: String,
        handle: ResizeHandle,
        start: WorldRect,
        start_screen: (f32, f32),
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SnapGuides {
    pub x: Option<f32>,
    pub y: Option<f32>,
}

pub struct SnapCandidates {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
}

#[derive(Debug)]
pub enum ProjectError {
    Config(crate::config::ConfigError),
    Session(crate::session::SessionError),
    NoProjectPath,
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::Config(e) => write!(f, "{e}"),
            ProjectError::Session(e) => write!(f, "{e}"),
            ProjectError::NoProjectPath => write!(f, "no project path set"),
        }
    }
}

impl std::error::Error for ProjectError {}

impl From<crate::config::ConfigError> for ProjectError {
    fn from(e: crate::config::ConfigError) -> Self {
        ProjectError::Config(e)
    }
}

impl From<crate::session::SessionError> for ProjectError {
    fn from(e: crate::session::SessionError) -> Self {
        ProjectError::Session(e)
    }
}

pub const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(30);

pub struct Engine {
    pub cfg: Config,
    pub registry: SourceRegistry,
    pub output_registry: OutputRegistry,
    pub ndi: Option<NdiDiscovery>,
    pub decklink: Option<DecklinkDiscovery>,
    #[cfg(target_os = "macos")]
    pub syphon: Option<SyphonDiscovery>,
    #[cfg(target_os = "macos")]
    pub avfoundation: Option<AvFoundationDiscovery>,
    #[cfg(target_os = "windows")]
    pub spout: Option<SpoutDiscovery>,
    comp: Option<Compositor>,
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    pub project_path: Option<PathBuf>,
    pub dirty: bool,
    last_saved_at: Instant,
    pub selected_layer_id: Option<LayerId>,
    pub expanded_layer_id: Option<LayerId>,
    pub drag_state: DragState,
    pub snap_guides: SnapGuides,
    pub view: ViewState,
    /// Warnings collected while loading the current project, e.g. sources
    /// whose protocol is unavailable on this platform.
    pub load_warnings: Vec<String>,
}

impl Engine {
    pub fn new_project() -> Self {
        let cfg = Config::default();
        let registry = SourceRegistry::new();
        let output_registry = OutputRegistry::new();

        // Start NDI discovery before restoring sources
        let ndi = NdiDiscovery::start();

        // Start DeckLink discovery
        let decklink = DecklinkDiscovery::start();

        // Start Syphon discovery on macOS
        #[cfg(target_os = "macos")]
        let syphon = Some(SyphonDiscovery::start());

        // Start AVFoundation device discovery on macOS
        #[cfg(target_os = "macos")]
        let avfoundation = Some(AvFoundationDiscovery::start());

        // Start Spout sender discovery on Windows
        #[cfg(target_os = "windows")]
        let spout = Some(SpoutDiscovery::start());

        let mut engine = Self {
            cfg,
            registry,
            output_registry,
            ndi: Some(ndi),
            decklink: Some(decklink),
            #[cfg(target_os = "macos")]
            syphon,
            #[cfg(target_os = "macos")]
            avfoundation,
            #[cfg(target_os = "windows")]
            spout,
            comp: None,
            device: None,
            queue: None,
            project_path: None,
            dirty: false,
            last_saved_at: Instant::now(),
            selected_layer_id: None,
            expanded_layer_id: None,
            drag_state: DragState::None,
            snap_guides: SnapGuides::default(),
            view: ViewState::new(),
            load_warnings: Vec::new(),
        };

        engine.rebuild_from_config();
        engine
    }

    fn rebuild_from_config(&mut self) {
        self.registry.clear();
        self.output_registry.clear();
        self.comp = None;
        self.load_warnings.clear();

        // Report protocols this platform cannot run. Their entries stay in the
        // config untouched (raw JSON preserved) and are written back on save.
        let unavailable: std::collections::BTreeSet<&str> = self
            .cfg
            .canvas
            .sources
            .iter()
            .map(|s| &s.protocol)
            .chain(self.cfg.canvas.outputs.iter().map(|o| &o.protocol))
            .filter_map(|p| match p {
                Protocol::Unknown(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        if !unavailable.is_empty() {
            self.load_warnings.push(format!(
                "Unavailable protocols in project: {}",
                unavailable.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }

        // Restore Test sources for all Test sources in the loaded config.
        // Each Test source gets a fresh dedicated test source.
        for (i, source) in self.cfg.canvas.sources.iter_mut().enumerate() {
            if source.uuid.is_empty() {
                source.uuid = uuid::Uuid::new_v4().to_string();
                self.dirty = true;
            }
            if source.name.is_empty() {
                source.name = format!("Source {}", i + 1);
                self.dirty = true;
            }
            if source.protocol == Protocol::Test {
                let test_config = match &source.config {
                    SourceConfig::Test(c) => Some(c.clone()),
                    _ => None,
                };
                let key = self.registry.add_test(test_config);
                source.source_ref = Some(key.source_ref);
                self.dirty = true;
            }
            // NDI and Syphon sources keep their source_ref; auto-connect happens in update()
        }

        // Seed demo layout if nothing was loaded
        if self.cfg.canvas.sources.is_empty() {
            let w = self.cfg.canvas.width as f32;
            let h = self.cfg.canvas.height as f32;
            for i in 0..2 {
                let key = self.registry.add_test(None);
                let col = i % 2;
                let row = i / 2;
                self.cfg.canvas.sources.push(Source::new(
                    format!("Source {}", i + 1),
                    Protocol::Test,
                    Some(key.source_ref),
                    col as f32 * w / 2.0,
                    row as f32 * h / 2.0,
                    (w / 2.0) as u32,
                    (h / 2.0) as u32,
                    i,
                    TextureMode::Fit,
                    false,
                    false,
                ));
            }
            self.dirty = true;
        }

        // Load outputs from config.
        for output in &mut self.cfg.canvas.outputs {
            if output.uuid.is_empty() {
                output.uuid = uuid::Uuid::new_v4().to_string();
                self.dirty = true;
            }
            match output.protocol {
                Protocol::Ndi => {
                    let id = output.uuid.clone();
                    let ndi_config = match &output.config {
                        OutputConfig::Ndi(c) => c.clone(),
                        _ => NdiOutputConfig::default(),
                    };
                    let name = if ndi_config.sender_name.is_empty() {
                        output.name.clone()
                    } else {
                        ndi_config.sender_name.clone()
                    };
                    self.output_registry.add_ndi(id, name, ndi_config, output.enabled);
                }
                Protocol::Decklink => {
                    let id = output.uuid.clone();
                    let decklink_config = match &output.config {
                        OutputConfig::Decklink(c) => c.clone(),
                        _ => DecklinkOutputConfig::default(),
                    };
                    let name = if decklink_config.device_name.is_empty() {
                        output.name.clone()
                    } else {
                        decklink_config.device_name.clone()
                    };
                    self.output_registry.add_decklink(id, name, decklink_config, output.enabled);
                }
                #[cfg(target_os = "macos")]
                Protocol::Syphon => {
                    let id = output.uuid.clone();
                    let syphon_config = match &output.config {
                        OutputConfig::Syphon(c) => c.clone(),
                        _ => SyphonOutputConfig::default(),
                    };
                    let name = if syphon_config.server_name.is_empty() {
                        output.name.clone()
                    } else {
                        syphon_config.server_name.clone()
                    };
                    self.output_registry.add_syphon(id, name, syphon_config, output.enabled);
                }
                #[cfg(target_os = "windows")]
                Protocol::Spout => {
                    let id = output.uuid.clone();
                    let spout_config = match &output.config {
                        OutputConfig::Spout(c) => c.clone(),
                        _ => SpoutOutputConfig::default(),
                    };
                    let name = if spout_config.sender_name.is_empty() {
                        output.name.clone()
                    } else {
                        spout_config.sender_name.clone()
                    };
                    self.output_registry.add_spout(id, name, spout_config, output.enabled);
                }
                _ => {}
            }
        }
    }

    pub fn open_project(&mut self, path: impl AsRef<std::path::Path>) -> Result<(), ProjectError> {
        let path = path.as_ref().to_path_buf();
        let cfg = Config::load_from(&path)?;
        self.cfg = cfg;
        self.dirty = false;
        self.rebuild_from_config();
        self.project_path = Some(path.clone());
        self.last_saved_at = Instant::now();
        self.dirty = false;
        crate::session::Session::default().set_last_project(&path)?;
        Ok(())
    }

    pub fn save_project(&mut self) -> Result<(), ProjectError> {
        let path = self.project_path.clone().ok_or(ProjectError::NoProjectPath)?;
        self.cfg.save_to(&path)?;
        self.last_saved_at = Instant::now();
        self.dirty = false;
        Ok(())
    }

    pub fn save_project_as(&mut self, path: impl AsRef<std::path::Path>) -> Result<(), ProjectError> {
        let path = path.as_ref().to_path_buf();
        self.cfg.save_to(&path)?;
        self.project_path = Some(path.clone());
        self.last_saved_at = Instant::now();
        self.dirty = false;
        crate::session::Session::default().set_last_project(&path)?;
        Ok(())
    }

    pub fn auto_save(&mut self) {
        if self.dirty
            && self.project_path.is_some()
            && self.last_saved_at.elapsed() >= AUTO_SAVE_INTERVAL
        {
            tracing::debug!("Auto-saving project");
            if let Err(e) = self.save_project() {
                tracing::error!("Auto-save failed: {e}");
            }
        }
    }


    pub fn update(&mut self) {
        // Auto-connect pending NDI sources when they appear in discovery
        if let Some(ref discovery) = self.ndi {
            let discovered = discovery.list();
            for source in &self.cfg.canvas.sources {
                if source.protocol == Protocol::Ndi
                    && let Some(ref source_ref) = source.source_ref
                {
                    let key = SourceKey::new(Protocol::Ndi, source_ref.clone());
                    if !self.registry.contains(&key)
                        && let Some(src) = discovered.iter().find(|s| &s.name == source_ref)
                    {
                        let config = match &source.config {
                            SourceConfig::Ndi(c) => c.clone(),
                            _ => NdiSourceConfig::default(),
                        };
                        self.registry.add_ndi(key, config, src.clone());
                        self.dirty = true;
                    }
                }
            }
        }

        // Auto-connect pending DeckLink sources when they appear in discovery
        if let Some(ref discovery) = self.decklink {
            let discovered = discovery.list();
            for source in &self.cfg.canvas.sources {
                if source.protocol == Protocol::Decklink
                    && let Some(ref source_ref) = source.source_ref
                {
                    let key = SourceKey::new(Protocol::Decklink, source_ref.clone());
                    if !self.registry.contains(&key)
                        && let Some(port) = discovered.iter().find(|p| &p.name == source_ref)
                    {
                        let config = match &source.config {
                            SourceConfig::Decklink(c) => c.clone(),
                            _ => DecklinkSourceConfig::default(),
                        };
                        self.registry.add_decklink(key, config, port.connections);
                        self.dirty = true;
                    }
                }
            }
        }

        // Auto-connect pending Syphon sources on macOS
        #[cfg(target_os = "macos")]
        {
            if let Some(ref discovery) = self.syphon {
                let discovered = discovery.list();
                for source in &self.cfg.canvas.sources {
                    if source.protocol == Protocol::Syphon
                        && let Some(ref source_ref) = source.source_ref
                    {
                        let key = SourceKey::new(Protocol::Syphon, source_ref.clone());
                        if !self.registry.contains(&key)
                            && discovered.iter().any(|s| s == source_ref)
                        {
                            let config = match &source.config {
                                SourceConfig::Syphon(c) => c.clone(),
                                _ => SyphonSourceConfig::default(),
                            };
                            self.registry.add_syphon(key, config);
                            self.dirty = true;
                        }
                    }
                }
            }
        }

        // Auto-connect pending AVFoundation sources on macOS
        #[cfg(target_os = "macos")]
        {
            if let Some(ref avf) = self.avfoundation {
                for source in &self.cfg.canvas.sources {
                    if source.protocol == Protocol::AvFoundation
                        && let Some(ref source_ref) = source.source_ref
                    {
                        let key = SourceKey::new(Protocol::AvFoundation, source_ref.clone());
                        if !self.registry.contains(&key)
                            && let Some(device) = avf.find_by_name(source_ref)
                        {
                            let mut config = match &source.config {
                                SourceConfig::AvFoundation(c) => c.clone(),
                                _ => AvFoundationSourceConfig::default(),
                            };
                            config.device_unique_id = device.unique_id.clone();
                            self.registry.add_avfoundation(key, config);
                            self.dirty = true;
                        }
                    }
                }
            }
        }

        // Auto-connect pending Spout sources on Windows
        #[cfg(target_os = "windows")]
        {
            if let Some(ref discovery) = self.spout {
                let discovered = discovery.list();
                for source in &self.cfg.canvas.sources {
                    if source.protocol == Protocol::Spout
                        && let Some(ref source_ref) = source.source_ref
                    {
                        let key = SourceKey::new(Protocol::Spout, source_ref.clone());
                        if !self.registry.contains(&key)
                            && discovered.iter().any(|s| s == source_ref)
                        {
                            let config = match &source.config {
                                SourceConfig::Spout(c) => c.clone(),
                                _ => SpoutSourceConfig::default(),
                            };
                            self.registry.add_spout(key, config);
                            self.dirty = true;
                        }
                    }
                }
            }
        }

        self.registry.apply_pending_restarts();
    }

    pub fn ensure_compositor(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) {
        if self.comp.is_none() {
            self.comp = Some(Compositor::new(device, queue, target_format));
        }
        if self.device.is_none() {
            self.device = Some(Arc::new(device.clone()));
        }
        if self.queue.is_none() {
            self.queue = Some(Arc::new(queue.clone()));
        }
    }

    pub fn build_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        panel_rect: &Rect,
        transform: (f32, f32, f32),
    ) -> Draw {
        let comp = self.comp.as_mut().expect("compositor not initialized");
        comp.build(
            device,
            queue,
            &self.cfg.canvas,
            &self.registry,
            panel_rect,
            transform,
            self.expanded_layer_id.as_deref(),
        )
    }

    pub fn render_outputs(&mut self) {
        if !self.output_registry.any_enabled() {
            return;
        }
        let Some(ref device) = self.device else {
            return;
        };
        let Some(ref queue) = self.queue else {
            return;
        };
        let comp = self.comp.as_mut().expect("compositor not initialized");
        comp.render_canvas(
            device,
            queue,
            &self.cfg.canvas,
            &self.registry,
            self.expanded_layer_id.as_deref(),
        );
        if let Some((texture, w, h)) = comp.canvas_texture() {
            self.output_registry
                .present_all(texture, w, h, device, queue);
        }
    }

    pub fn shared(&self) -> Option<std::sync::Arc<crate::compositor::Shared>> {
        self.comp.as_ref().map(|c| c.shared.clone())
    }

    pub fn cleanup_orphaned_sources(&mut self) {
        let active_keys: Vec<SourceKey> = self
            .cfg
            .canvas
            .sources
            .iter()
            .filter_map(|l| {
                l.source_ref.clone()
                    .map(|source_ref| SourceKey::new(l.protocol.clone(), source_ref))
            })
            .collect();
        self.registry.cleanup_orphaned_sources(&active_keys);
    }

    pub fn add_layer(&mut self) -> String {
        let key = self.registry.add_test(None);
        let num = self.cfg.canvas.sources.len() + 1;
        let source = Source::new(
            format!("Source {num}"),
            Protocol::Test,
            Some(key.source_ref),
            self.cfg.canvas.width as f32 * 0.25,
            self.cfg.canvas.height as f32 * 0.25,
            self.cfg.canvas.width / 2,
            self.cfg.canvas.height / 2,
            self.cfg.canvas.sources.len() as i32,
            TextureMode::Fit,
            false,
            false,
        );
        let uuid = source.uuid.clone();
        self.cfg.canvas.sources.push(source);
        self.dirty = true;
        uuid
    }

    pub fn remove_layer(&mut self, uuid: &str) {
        self.cfg.canvas.sources.retain(|l| l.uuid != uuid);
        if self.selected_layer_id.as_deref() == Some(uuid) {
            self.selected_layer_id = None;
        }
        if self.expanded_layer_id.as_deref() == Some(uuid) {
            self.expanded_layer_id = None;
        }
        self.dirty = true;
    }

    /// Persist a runtime protocol config edit. The runtime source is shared by
    /// every quad bound to the same (protocol, source_ref), so the edit is
    /// written to all of them — one source, one config.
    pub fn sync_source_config(&mut self, layer_uuid: &str, config: SourceConfig) {
        let Some((protocol, source_ref)) = self
            .cfg
            .canvas
            .sources
            .iter()
            .find(|s| s.uuid == layer_uuid)
            .map(|s| (s.protocol.clone(), s.source_ref.clone()))
        else {
            return;
        };
        for quad in self.cfg.canvas.sources.iter_mut() {
            let bound = quad.uuid == layer_uuid
                || (source_ref.is_some()
                    && quad.protocol == protocol
                    && quad.source_ref == source_ref);
            if bound {
                quad.config = config.clone();
            }
        }
        self.dirty = true;
    }

    /// Persisted config of the quad on `protocol` that references `source_ref`
    fn layer_config(&self, protocol: &Protocol, source_ref: &str) -> Option<&SourceConfig> {
        return self
            .cfg
            .canvas
            .sources
            .iter()
            .find(|s| s.protocol == *protocol && s.source_ref.as_deref() == Some(source_ref))
            .map(|s| &s.config);
    }

    pub fn connect(&mut self, protocol: Protocol, name: &str) {
        let key = SourceKey::new(protocol.clone(), name.to_string());
        match protocol {
            // Test sources are created locally, never connected to.
            Protocol::Test => {}
            Protocol::Ndi => {
                if let Some(ref discovery) = self.ndi
                    && let Some(src) = discovery.find_by_name(name)
                {
                    let config = match self.layer_config(&Protocol::Ndi, name) {
                        Some(SourceConfig::Ndi(c)) => c.clone(),
                        _ => NdiSourceConfig::default(),
                    };
                    self.registry.add_ndi(key, config, src);
                }
            }
            Protocol::Decklink => {
                let config = match self.layer_config(&Protocol::Decklink, name) {
                    Some(SourceConfig::Decklink(c)) => c.clone(),
                    _ => DecklinkSourceConfig::default(),
                };
                let supported_connections = self
                    .decklink
                    .as_ref()
                    .and_then(|d| d.find_by_name(name))
                    .map(|p| p.connections)
                    .unwrap_or_default();
                self.registry.add_decklink(key, config, supported_connections);
            }
            #[cfg(target_os = "macos")]
            Protocol::Syphon => {
                let config = match self.layer_config(&Protocol::Syphon, name) {
                    Some(SourceConfig::Syphon(c)) => c.clone(),
                    _ => SyphonSourceConfig::default(),
                };
                self.registry.add_syphon(key, config);
            }
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation => {
                if let Some(device) = self.avfoundation.as_ref().and_then(|d| d.find_by_name(name)) {
                    let config = AvFoundationSourceConfig {
                        device_unique_id: device.unique_id.clone(),
                    };
                    self.registry.add_avfoundation(key, config);
                }
            }
            #[cfg(target_os = "windows")]
            Protocol::Spout => {
                let config = match self.layer_config(&Protocol::Spout, name) {
                    Some(SourceConfig::Spout(c)) => c.clone(),
                    _ => SpoutSourceConfig::default(),
                };
                self.registry.add_spout(key, config);
            }
            // Unavailable protocols have no runtime source to connect.
            Protocol::Unknown(_) => {}
        }
    }

    pub fn add_output(&mut self, protocol: Protocol, name: String, config: OutputConfig) -> String {
        let output = crate::config::Output::new(name.clone(), protocol.clone(), true, config.clone());
        let id = output.uuid.clone();
        self.cfg.canvas.outputs.push(output);

        match (protocol, config) {
            (Protocol::Ndi, OutputConfig::Ndi(c)) => {
                let output_name = if c.sender_name.is_empty() { name } else { c.sender_name.clone() };
                self.output_registry.add_ndi(id.clone(), output_name, c, true);
            }
            (Protocol::Decklink, OutputConfig::Decklink(c)) => {
                let output_name = if c.device_name.is_empty() { name } else { c.device_name.clone() };
                self.output_registry.add_decklink(id.clone(), output_name, c, true);
            }
            #[cfg(target_os = "macos")]
            (Protocol::Syphon, OutputConfig::Syphon(c)) => {
                let output_name = if c.server_name.is_empty() { name } else { c.server_name.clone() };
                self.output_registry.add_syphon(id.clone(), output_name, c, true);
            }
            #[cfg(target_os = "windows")]
            (Protocol::Spout, OutputConfig::Spout(c)) => {
                let output_name = if c.sender_name.is_empty() { name } else { c.sender_name.clone() };
                self.output_registry.add_spout(id.clone(), output_name, c, true);
            }
            _ => {}
        }

        self.dirty = true;
        id
    }

    pub fn remove_output(&mut self, uuid: &str) {
        self.output_registry.remove(&uuid.to_string());
        self.cfg.canvas.outputs.retain(|o| o.uuid != uuid);
        self.dirty = true;
    }

    pub fn set_output_enabled(&mut self, uuid: &str, enabled: bool) {
        if let Some(output) = self.cfg.canvas.outputs.iter_mut().find(|o| o.uuid == uuid) {
            output.enabled = enabled;
        }
        if let Some(out) = self.output_registry.get_mut(&uuid.to_string()) {
            out.set_enabled(enabled);
        } else if enabled {
            tracing::info!("output {uuid}: runtime missing while enabling, recreating");
            self.restart_output(uuid);
        }
        self.dirty = true;
    }

    #[allow(dead_code)]
    pub fn outputs_enabled(&self) -> bool {
        self.output_registry.any_enabled()
    }

    pub fn restart_output(&mut self, uuid: &str) {
        let output = match self.cfg.canvas.outputs.iter().find(|o| o.uuid == uuid) {
            Some(o) => o.clone(),
            None => return,
        };

        self.output_registry.remove(&uuid.to_string());

        match (&output.protocol, &output.config) {
            (Protocol::Ndi, OutputConfig::Ndi(c)) => {
                let name = if c.sender_name.is_empty() { output.name.clone() } else { c.sender_name.clone() };
                self.output_registry.add_ndi(uuid.to_string(), name, c.clone(), output.enabled);
            }
            (Protocol::Decklink, OutputConfig::Decklink(c)) => {
                let name = if c.device_name.is_empty() { output.name.clone() } else { c.device_name.clone() };
                self.output_registry.add_decklink(uuid.to_string(), name, c.clone(), output.enabled);
            }
            #[cfg(target_os = "macos")]
            (Protocol::Syphon, OutputConfig::Syphon(c)) => {
                let name = if c.server_name.is_empty() { output.name.clone() } else { c.server_name.clone() };
                self.output_registry.add_syphon(uuid.to_string(), name, c.clone(), output.enabled);
            }
            #[cfg(target_os = "windows")]
            (Protocol::Spout, OutputConfig::Spout(c)) => {
                let name = if c.sender_name.is_empty() { output.name.clone() } else { c.sender_name.clone() };
                self.output_registry.add_spout(uuid.to_string(), name, c.clone(), output.enabled);
            }
            _ => {}
        }

        self.dirty = true;
    }
}

// UI/canvas engine methods
impl Engine {
    pub fn display_transform(&self, panel_rect: &Rect) -> (f32, f32, f32) {
        let (base_scale, base_ox, base_oy) =
            compositor::canvas_transform(&self.cfg.canvas, panel_rect);
        (
            base_scale * self.view.zoom,
            base_ox + self.view.pan.x,
            base_oy + self.view.pan.y,
        )
    }

    /// Base transform that fits the canvas into the panel without any user pan/zoom.
    pub fn default_transform(&self, panel_rect: &Rect) -> (f32, f32, f32) {
        compositor::canvas_transform(&self.cfg.canvas, panel_rect)
    }

    pub fn recenter_view(&mut self, panel_rect: &Rect) {
        let canvas = &self.cfg.canvas;
        let (mut min_x, mut min_y) = (0.0_f32, 0.0_f32);
        let (mut max_x, mut max_y) = (canvas.width as f32, canvas.height as f32);
        for source in &canvas.sources {
            min_x = min_x.min(source.x).min(source.x + source.width as f32);
            min_y = min_y.min(source.y).min(source.y + source.height as f32);
            max_x = max_x.max(source.x).max(source.x + source.width as f32);
            max_y = max_y.max(source.y).max(source.y + source.height as f32);
        }
        let bbox_w = (max_x - min_x).max(1.0);
        let bbox_h = (max_y - min_y).max(1.0);
        let (base_scale, base_ox, base_oy) = compositor::canvas_transform(canvas, panel_rect);
        let target_scale = (panel_rect.width() / bbox_w).min(panel_rect.height() / bbox_h) * 0.9;
        self.view.zoom = (target_scale / base_scale).clamp(MIN_ZOOM, MAX_ZOOM);
        let display_scale = base_scale * self.view.zoom;
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        self.view.pan.x = panel_rect.width() / 2.0 - base_ox - cx * display_scale;
        self.view.pan.y = panel_rect.height() / 2.0 - base_oy - cy * display_scale;
    }

    pub fn zoom_view(&mut self, panel_rect: &Rect, factor: f32) {
        let (base_scale, base_ox, base_oy) =
            compositor::canvas_transform(&self.cfg.canvas, panel_rect);
        let old_zoom = self.view.zoom;
        let new_zoom = (old_zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let center = egui::vec2(panel_rect.w / 2.0, panel_rect.h / 2.0);
        let world_c = (center
            - egui::vec2(base_ox + self.view.pan.x, base_oy + self.view.pan.y))
            / (base_scale * old_zoom);
        self.view.zoom = new_zoom;
        self.view.pan = egui::vec2(
            center.x - base_ox - world_c.x * base_scale * new_zoom,
            center.y - base_oy - world_c.y * base_scale * new_zoom,
        );
    }

    pub fn expand_source(&mut self, uuid: String) {
        self.expanded_layer_id = Some(uuid);
    }

    pub fn clear_expanded_source(&mut self) {
        self.expanded_layer_id = None;
    }

    pub fn expanded_layer_id(&self) -> Option<&str> {
        self.expanded_layer_id.as_deref()
    }

    pub fn select_next_source(&mut self) {
        if self.cfg.canvas.sources.is_empty() {
            self.selected_layer_id = None;
            return;
        }
        let current_index = self
            .selected_layer_id
            .as_ref()
            .and_then(|uuid| self.cfg.canvas.sources.iter().position(|s| &s.uuid == uuid));
        let next_index = match current_index {
            Some(i) => (i + 1) % self.cfg.canvas.sources.len(),
            None => 0,
        };
        self.selected_layer_id = Some(self.cfg.canvas.sources[next_index].uuid.clone());
    }

    pub fn select_previous_source(&mut self) {
        if self.cfg.canvas.sources.is_empty() {
            self.selected_layer_id = None;
            return;
        }
        let current_index = self
            .selected_layer_id
            .as_ref()
            .and_then(|uuid| self.cfg.canvas.sources.iter().position(|s| &s.uuid == uuid));
        let prev_index = match current_index {
            Some(i) => {
                if i == 0 {
                    self.cfg.canvas.sources.len() - 1
                } else {
                    i - 1
                }
            }
            None => self.cfg.canvas.sources.len() - 1,
        };
        self.selected_layer_id = Some(self.cfg.canvas.sources[prev_index].uuid.clone());
    }

    pub fn nudge_selected_source(&mut self, dx: f32, dy: f32) {
        if self.expanded_layer_id.is_some() {
            return;
        }
        let Some(uuid) = self.selected_layer_id.clone() else { return };
        if let Some(source) = self.cfg.canvas.sources.iter_mut().find(|s| s.uuid == uuid) {
            source.x += dx;
            source.y += dy;
            self.dirty = true;
        }
    }

    pub fn move_layer(&mut self, from_index: usize, to_index: usize) {
        let len = self.cfg.canvas.sources.len();
        if from_index == to_index || from_index >= len || to_index >= len {
            return;
        }
        let source = self.cfg.canvas.sources.remove(from_index);
        self.cfg.canvas.sources.insert(to_index, source);
        self.dirty = true;
    }

    pub fn drag_layer(&mut self, uuid: &str, delta: (f32, f32), panel_rect: &Rect) {
        let (scale, _, _) = self.display_transform(panel_rect);
        let snap_threshold = SNAP_THRESHOLD / scale;
        let break_threshold = SNAP_BREAK_THRESHOLD / scale;
        let candidates = self.snap_candidates(uuid);

        if let Some(source) = self.cfg.canvas.sources.iter_mut().find(|l| l.uuid == uuid) {
            let (dx, dy) = delta;
            let proposed_x = source.x + dx / scale;
            let proposed_y = source.y + dy / scale;

            let left = proposed_x;
            let right = proposed_x + source.width as f32;
            let top = proposed_y;
            let bottom = proposed_y + source.height as f32;

            let current_x = self.snap_guides.x;
            let current_y = self.snap_guides.y;
            let snap_left = Self::snap_value(
                left,
                &candidates.x,
                snap_threshold,
                break_threshold,
                current_x,
            );
            let snap_right = Self::snap_value(
                right,
                &candidates.x,
                snap_threshold,
                break_threshold,
                current_x,
            );
            let snap_top = Self::snap_value(
                top,
                &candidates.y,
                snap_threshold,
                break_threshold,
                current_y,
            );
            let snap_bottom = Self::snap_value(
                bottom,
                &candidates.y,
                snap_threshold,
                break_threshold,
                current_y,
            );

            let (x_offset, guide_x) = match (snap_left, snap_right) {
                (Some(l), Some(r)) => {
                    if (l - left).abs() < (r - right).abs() {
                        (l - left, l)
                    } else {
                        (r - right, r)
                    }
                }
                (Some(l), None) => (l - left, l),
                (None, Some(r)) => (r - right, r),
                (None, None) => (0.0, 0.0),
            };
            let (y_offset, guide_y) = match (snap_top, snap_bottom) {
                (Some(t), Some(b)) => {
                    if (t - top).abs() < (b - bottom).abs() {
                        (t - top, t)
                    } else {
                        (b - bottom, b)
                    }
                }
                (Some(t), None) => (t - top, t),
                (None, Some(b)) => (b - bottom, b),
                (None, None) => (0.0, 0.0),
            };

            source.x = (proposed_x + x_offset).round();
            source.y = (proposed_y + y_offset).round();
            self.snap_guides.x = if x_offset != 0.0 { Some(guide_x) } else { None };
            self.snap_guides.y = if y_offset != 0.0 { Some(guide_y) } else { None };
            self.dirty = true;
        }
    }

    pub fn layer_rect_world(&self, uuid: &str) -> Option<WorldRect> {
        self.cfg
            .canvas
            .sources
            .iter()
            .find(|l| l.uuid == uuid)
            .map(|l| WorldRect {
                x: l.x,
                y: l.y,
                w: l.width as f32,
                h: l.height as f32,
            })
    }

    pub fn snap_candidates(&self, exclude_uuid: &str) -> SnapCandidates {
        let canvas = &self.cfg.canvas;
        let mut x = vec![0.0, canvas.width as f32];
        let mut y = vec![0.0, canvas.height as f32];
        for source in &canvas.sources {
            if source.uuid == exclude_uuid {
                continue;
            }
            x.push(source.x);
            x.push(source.x + source.width as f32);
            y.push(source.y);
            y.push(source.y + source.height as f32);
        }
        SnapCandidates { x, y }
    }

    fn snap_value(
        value: f32,
        candidates: &[f32],
        snap_threshold: f32,
        break_threshold: f32,
        current: Option<f32>,
    ) -> Option<f32> {
        let mut best = None;
        let mut best_dist = f32::INFINITY;
        for &c in candidates {
            let dist = (c - value).abs();
            if dist < best_dist {
                best_dist = dist;
                best = Some(c);
            }
        }
        if best_dist < snap_threshold {
            return best;
        }
        // Hysteresis: stay snapped to the current guide until the mouse moves
        // past the larger break threshold.
        if let Some(curr) = current {
            let dist = (curr - value).abs();
            if dist < break_threshold {
                return Some(curr);
            }
        }
        None
    }

    pub fn hit_test(&self, panel_rect: &Rect, pos: (f32, f32)) -> Option<String> {
        let canvas = &self.cfg.canvas;
        let (scale, offset_x, offset_y) = self.display_transform(panel_rect);
        let cx = panel_rect.x + offset_x;
        let cy = panel_rect.y + offset_y;

        let mut sources: Vec<_> = canvas.sources.iter().collect();
        sources.sort_by_key(|l| -l.z);

        let (px, py) = pos;
        for source in sources {
            let lx = cx + source.x * scale;
            let ly = cy + source.y * scale;
            let lw = source.width as f32 * scale;
            let lh = source.height as f32 * scale;
            if px >= lx && px <= lx + lw && py >= ly && py <= ly + lh {
                return Some(source.uuid.clone());
            }
        }
        None
    }

    pub fn hit_test_resize_handle(
        &self,
        panel_rect: &Rect,
        pos: (f32, f32),
    ) -> Option<(String, ResizeHandle)> {
        let uuid = self.selected_layer_id.as_ref()?;
        let source = self.cfg.canvas.sources.iter().find(|l| &l.uuid == uuid)?;
        let (scale, offset_x, offset_y) = self.display_transform(panel_rect);
        let cx = panel_rect.x + offset_x;
        let cy = panel_rect.y + offset_y;
        let lx = cx + source.x * scale;
        let ly = cy + source.y * scale;
        let lw = source.width as f32 * scale;
        let lh = source.height as f32 * scale;
        let right = lx + lw;
        let bottom = ly + lh;
        let (px, py) = pos;
        const H: f32 = 8.0; // hit radius in screen points

        // Corners take priority over edges.
        if (px - lx).abs() <= H && (py - ly).abs() <= H {
            return Some((uuid.clone(), ResizeHandle::TopLeft));
        }
        if (px - right).abs() <= H && (py - ly).abs() <= H {
            return Some((uuid.clone(), ResizeHandle::TopRight));
        }
        if (px - lx).abs() <= H && (py - bottom).abs() <= H {
            return Some((uuid.clone(), ResizeHandle::BottomLeft));
        }
        if (px - right).abs() <= H && (py - bottom).abs() <= H {
            return Some((uuid.clone(), ResizeHandle::BottomRight));
        }

        // Edges.
        if (py - ly).abs() <= H && px >= lx && px <= right {
            return Some((uuid.clone(), ResizeHandle::Top));
        }
        if (py - bottom).abs() <= H && px >= lx && px <= right {
            return Some((uuid.clone(), ResizeHandle::Bottom));
        }
        if (px - lx).abs() <= H && py >= ly && py <= bottom {
            return Some((uuid.clone(), ResizeHandle::Left));
        }
        if (px - right).abs() <= H && py >= ly && py <= bottom {
            return Some((uuid.clone(), ResizeHandle::Right));
        }

        None
    }

    pub fn resize_layer(
        &mut self,
        uuid: &str,
        handle: ResizeHandle,
        start: WorldRect,
        delta_screen: (f32, f32),
        panel_rect: &Rect,
    ) {
        let (scale, _, _) = self.display_transform(panel_rect);
        let dx = delta_screen.0 / scale;
        let dy = delta_screen.1 / scale;
        let snap_threshold = SNAP_THRESHOLD / scale;
        let break_threshold = SNAP_BREAK_THRESHOLD / scale;
        let candidates = self.snap_candidates(uuid);

        if let Some(source) = self.cfg.canvas.sources.iter_mut().find(|l| l.uuid == uuid) {
            let mut x = start.x;
            let mut y = start.y;
            let mut w = start.w;
            let mut h = start.h;
            let mut guide_x = None;
            let mut guide_y = None;

            match handle {
                ResizeHandle::Left => {
                    let proposed = x + dx;
                    if let Some(snap) = Self::snap_value(
                        proposed,
                        &candidates.x,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.x,
                    ) {
                        guide_x = Some(snap);
                        x = snap.min(start.x + start.w - 1.0);
                        w = (start.x + start.w) - x;
                    } else {
                        let new_x = x + dx;
                        let new_w = (x + w) - new_x;
                        if new_w >= 1.0 {
                            x = new_x;
                            w = new_w;
                        } else {
                            x = x + w - 1.0;
                            w = 1.0;
                        }
                    }
                }
                ResizeHandle::Right => {
                    let proposed = x + w + dx;
                    if let Some(snap) = Self::snap_value(
                        proposed,
                        &candidates.x,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.x,
                    ) {
                        guide_x = Some(snap);
                        w = (snap - x).max(1.0);
                    } else {
                        w = (w + dx).max(1.0);
                    }
                }
                ResizeHandle::Top => {
                    let proposed = y + dy;
                    if let Some(snap) = Self::snap_value(
                        proposed,
                        &candidates.y,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.y,
                    ) {
                        guide_y = Some(snap);
                        y = snap.min(start.y + start.h - 1.0);
                        h = (start.y + start.h) - y;
                    } else {
                        let new_y = y + dy;
                        let new_h = (y + h) - new_y;
                        if new_h >= 1.0 {
                            y = new_y;
                            h = new_h;
                        } else {
                            y = y + h - 1.0;
                            h = 1.0;
                        }
                    }
                }
                ResizeHandle::Bottom => {
                    let proposed = y + h + dy;
                    if let Some(snap) = Self::snap_value(
                        proposed,
                        &candidates.y,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.y,
                    ) {
                        guide_y = Some(snap);
                        h = (snap - y).max(1.0);
                    } else {
                        h = (h + dy).max(1.0);
                    }
                }
                _ => {
                    // Corner drag: preserve aspect ratio by projecting the moving
                    // corner onto the diagonal from the fixed opposite corner, then
                    // round width and derive height from the original aspect so the
                    // integer dimensions stay proportional.
                    let (fx, fy) = match handle {
                        ResizeHandle::TopLeft => (x + w, y + h),
                        ResizeHandle::TopRight => (x, y + h),
                        ResizeHandle::BottomRight => (x, y),
                        ResizeHandle::BottomLeft => (x + w, y),
                        _ => unreachable!(),
                    };
                    let mx0 = start.x + start.w - (fx - start.x); // start moving corner x
                    let my0 = start.y + start.h - (fy - start.y); // start moving corner y
                    let diag_x = mx0 - fx;
                    let diag_y = my0 - fy;
                    let denom = diag_x * diag_x + diag_y * diag_y;
                    if denom > 0.0 {
                        let t = ((mx0 + dx - fx) * diag_x + (my0 + dy - fy) * diag_y) / denom;
                        let min_t = (1.0 / start.w).max(1.0 / start.h);
                        let t = t.max(min_t);
                        let mut mx = fx + t * diag_x;
                        let mut my = fy + t * diag_y;

                        // Snap the moving corner to candidates, preferring the closer axis.
                        let snap_mx = Self::snap_value(
                            mx,
                            &candidates.x,
                            snap_threshold,
                            break_threshold,
                            self.snap_guides.x,
                        );
                        let snap_my = Self::snap_value(
                            my,
                            &candidates.y,
                            snap_threshold,
                            break_threshold,
                            self.snap_guides.y,
                        );
                        let dist_x = snap_mx.map(|v| (v - mx).abs());
                        let dist_y = snap_my.map(|v| (v - my).abs());
                        match (dist_x, dist_y) {
                            (Some(dx_), Some(dy_)) => {
                                if dx_ < dy_ {
                                    mx = snap_mx.unwrap();
                                    guide_x = Some(mx);
                                } else {
                                    my = snap_my.unwrap();
                                    mx = fx + (my - fy) * diag_x / diag_y;
                                    guide_y = Some(my);
                                }
                            }
                            (Some(_), None) => {
                                mx = snap_mx.unwrap();
                                guide_x = Some(mx);
                            }
                            (None, Some(_)) => {
                                my = snap_my.unwrap();
                                mx = fx + (my - fy) * diag_x / diag_y;
                                guide_y = Some(my);
                            }
                            (None, None) => {}
                        }

                        let new_w = (mx - fx).abs().round().max(1.0);
                        let new_h = (new_w * start.h / start.w).round().max(1.0);
                        x = match handle {
                            ResizeHandle::TopLeft | ResizeHandle::BottomLeft => fx - new_w,
                            _ => fx,
                        };
                        y = match handle {
                            ResizeHandle::TopLeft | ResizeHandle::TopRight => fy - new_h,
                            _ => fy,
                        };
                        w = new_w;
                        h = new_h;
                    }
                }
            }

            source.x = x.round();
            source.y = y.round();
            source.width = w.max(1.0).round() as u32;
            source.height = h.max(1.0).round() as u32;
            self.snap_guides = SnapGuides {
                x: guide_x,
                y: guide_y,
            };
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine(canvas: crate::config::Canvas) -> Engine {
        Engine {
            cfg: crate::config::Config { canvas },
            registry: SourceRegistry::new(),
            output_registry: OutputRegistry::new(),
            ndi: None,
            decklink: None,
            #[cfg(target_os = "macos")]
            syphon: None,
            #[cfg(target_os = "macos")]
            avfoundation: None,
            #[cfg(target_os = "windows")]
            spout: None,
            comp: None,
            device: None,
            queue: None,
            project_path: None,
            dirty: false,
            last_saved_at: Instant::now(),
            selected_layer_id: None,
            expanded_layer_id: None,
            drag_state: DragState::None,
            snap_guides: SnapGuides::default(),
            view: ViewState::new(),
            load_warnings: Vec::new(),
        }
    }

    /// A project authored where unavailable protocols exist (Syphon on
    /// Windows, an unknown protocol anywhere) must load, warn in the status
    /// bar, and never spawn a runtime source for it.
    #[test]
    fn load_project_with_unavailable_protocol_warns_and_skips_runtime() {
        let json = r#"{
            "canvas": {
                "width":1920, "height":1080,
                "sources": [
                    {"uuid":"u1","name":"Spout In","protocol":"fake","source_ref":"Spout1",
                     "x":0.0,"y":0.0,"width":640,"height":360,"z":0,"mode":"Fit",
                     "config":{"protocol":"fake","name":"Game"}},
                    {"uuid":"u2","name":"Bars","protocol":"Test","source_ref":"Test A",
                     "x":0.0,"y":0.0,"width":640,"height":360,"z":1,"mode":"Fit"}
                ],
                "outputs":[
                    {"uuid":"o1","name":"Spout Out","protocol":"fake","enabled":true,
                     "config":{"protocol":"fake","name":"Program"}}
                ]
            }
        }"#;

        let dir = std::env::temp_dir().join(format!("multiviewer-unavailable-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("show.multiviewer");
        std::fs::write(&path, json).unwrap();

        let cfg = crate::config::Config::load_from(&path).unwrap();
        let mut engine = test_engine(cfg.canvas);
        engine.rebuild_from_config();

        // Both entries survive, the unavailable config is kept verbatim.
        assert_eq!(engine.cfg.canvas.sources.len(), 2);
        assert_eq!(engine.cfg.canvas.outputs.len(), 1);
        let SourceConfig::Unknown(v) = &engine.cfg.canvas.sources[0].config else {
            panic!("expected Unknown config")
        };
        assert_eq!(v.get("name"), Some(&serde_json::json!("Game")));

        // Only the Test source gets a runtime instance. The unavailable quad's
        // key is distinct from any live source that happens to share its ref.
        assert_eq!(engine.registry.iter().count(), 1);
        let unavailable_key =
            SourceKey::new(Protocol::Unknown("fake".to_string()), "Spout1".to_string());
        assert!(engine.registry.get(&unavailable_key).is_none());
        let test_key = SourceKey::new(
            Protocol::Test,
            engine.cfg.canvas.sources[1].source_ref.clone().unwrap(),
        );
        assert!(engine.registry.get(&test_key).is_some());

        // The status bar picks this up from load_warnings.
        assert_eq!(engine.load_warnings.len(), 1);
        assert!(engine.load_warnings[0].contains("fake"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expand_and_clear_source() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(
            "L1".into(),
            Protocol::Test,
            None,
            100.0,
            100.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let uuid = engine.cfg.canvas.sources[0].uuid.clone();

        assert!(engine.expanded_layer_id().is_none());
        engine.expand_source(uuid.clone());
        assert_eq!(engine.expanded_layer_id(), Some(uuid.as_str()));
        engine.clear_expanded_source();
        assert!(engine.expanded_layer_id().is_none());
    }

    #[test]
    fn removing_expanded_source_clears_it() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(
            "L1".into(),
            Protocol::Test,
            None,
            100.0,
            100.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let uuid = engine.cfg.canvas.sources[0].uuid.clone();
        engine.expand_source(uuid.clone());

        engine.remove_layer(&uuid);
        assert!(engine.expanded_layer_id().is_none());
    }

    #[test]
    fn display_transform_is_base_at_default_zoom() {
        let engine = test_engine(crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        });
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        };
        let (scale, ox, oy) = engine.display_transform(&panel);
        let (base_scale, base_ox, base_oy) =
            compositor::canvas_transform(&engine.cfg.canvas, &panel);
        assert!((scale - base_scale).abs() < 1e-3);
        assert!((ox - base_ox).abs() < 1e-3);
        assert!((oy - base_oy).abs() < 1e-3);
    }

    #[test]
    fn recenter_fits_canvas_with_margin() {
        let mut engine = test_engine(crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        });
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        };
        engine.recenter_view(&panel);
        let (scale, ox, oy) = engine.display_transform(&panel);
        let expected_scale = (panel.w / 1920.0).min(panel.h / 1080.0) * 0.9;
        assert!((scale - expected_scale).abs() < 1e-3);
        assert!((ox - (panel.w - 1920.0 * scale) / 2.0).abs() < 1e-3);
        assert!((oy - (panel.h - 1080.0 * scale) / 2.0).abs() < 1e-3);
    }

    #[test]
    fn recenter_expands_to_include_layers() {
        let mut canvas = crate::config::Canvas {
            width: 100,
            height: 100,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(

            "L1".into(),
            Protocol::Test,
            None,
            -50.0,
            -50.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        canvas.sources.push(Source::new(
            "L2".into(),
            Protocol::Test,
            None,
            200.0,
            200.0,
            100,
            100,
            1,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 400.0,
        };
        engine.recenter_view(&panel);
        let (scale, _ox, _oy) = engine.display_transform(&panel);
        let bbox_w = 350.0;
        let bbox_h = 350.0;
        let expected_scale = (panel.w / bbox_w).min(panel.h / bbox_h) * 0.9;
        assert!((scale - expected_scale).abs() < 1e-3);
    }

    #[test]
    fn resize_layer_handles_corners_and_edges() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(

            "L1".into(),
            Protocol::Test,
            None,
            100.0,
            100.0,
            200,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let uuid = engine.cfg.canvas.sources[0].uuid.clone();
        let start = engine.layer_rect_world(&uuid).unwrap();

        // Edge resize: drag right edge 30 px to the right.
        engine.resize_layer(&uuid, ResizeHandle::Right, start, (30.0, 0.0), &panel);
        let source = &engine.cfg.canvas.sources[0];
        assert_eq!(source.x, 100.0);
        assert_eq!(source.y, 100.0);
        assert_eq!(source.width, 230);
        assert_eq!(source.height, 100);

        // Edge resize: drag top edge 20 px up.
        let start = engine.layer_rect_world(&uuid).unwrap();
        engine.resize_layer(&uuid, ResizeHandle::Top, start, (0.0, -20.0), &panel);
        let source = &engine.cfg.canvas.sources[0];
        assert_eq!(source.x, 100.0);
        assert_eq!(source.y, 80.0);
        assert_eq!(source.width, 230);
        assert_eq!(source.height, 120);

        // Corner resize: drag bottom-right along the diagonal.
        let start = engine.layer_rect_world(&uuid).unwrap();
        engine.resize_layer(
            &uuid,
            ResizeHandle::BottomRight,
            start,
            (50.0, 50.0 * 120.0 / 230.0),
            &panel,
        );
        let source = &engine.cfg.canvas.sources[0];
        let aspect = source.width as f32 / source.height as f32;
        // Integer dimensions can't match the exact float aspect; allow ~1% rounding error.
        assert!(
            (aspect - 230.0 / 120.0).abs() < 0.02,
            "aspect should be preserved, got {aspect}"
        );
        assert_eq!(source.x, 100.0);
        assert_eq!(source.y, 80.0);
    }

    #[test]
    fn drag_layer_snaps_to_canvas_edge() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(

            "L1".into(),
            Protocol::Test,
            None,
            15.0,
            100.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let uuid = engine.cfg.canvas.sources[0].uuid.clone();
        // Drag left by 14 px: left edge moves from 15 to 1, within the 2 px snap threshold of 0.
        engine.drag_layer(&uuid, (-14.0, 0.0), &panel);
        let source = &engine.cfg.canvas.sources[0];
        assert_eq!(source.x, 0.0);
        assert!(engine.snap_guides.x.is_some());
    }

    #[test]
    fn resize_layer_snaps_to_other_layer_edge() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(

            "L1".into(),
            Protocol::Test,
            None,
            100.0,
            100.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        canvas.sources.push(Source::new(
            "L2".into(),
            Protocol::Test,
            None,
            300.0,
            100.0,
            100,
            100,
            1,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let uuid = engine.cfg.canvas.sources[0].uuid.clone();
        let start = engine.layer_rect_world(&uuid).unwrap();
        // Drag L1's right edge to 299: should snap to L2's left edge at 300.
        engine.resize_layer(&uuid, ResizeHandle::Right, start, (99.0, 0.0), &panel);
        let source = &engine.cfg.canvas.sources[0];
        assert_eq!(source.width, 200);
        assert!(engine.snap_guides.x.is_some());
    }

    #[test]
    fn drag_layer_hysteresis_releases_after_break_threshold() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(

            "L1".into(),
            Protocol::Test,
            None,
            15.0,
            100.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let uuid = engine.cfg.canvas.sources[0].uuid.clone();

        // Snap left edge to the canvas edge at 0.
        engine.drag_layer(&uuid, (-14.0, 0.0), &panel);
        assert_eq!(engine.cfg.canvas.sources[0].x, 0.0);
        assert!(engine.snap_guides.x.is_some());

        // Move 1 px back: stays snapped within the 20 px break threshold.
        engine.drag_layer(&uuid, (1.0, 0.0), &panel);
        assert_eq!(engine.cfg.canvas.sources[0].x, 0.0);
        assert!(engine.snap_guides.x.is_some());

        // Move 25 px past the snap point: breaks free.
        engine.drag_layer(&uuid, (25.0, 0.0), &panel);
        assert_eq!(engine.cfg.canvas.sources[0].x, 25.0);
        assert!(engine.snap_guides.x.is_none());
    }

    #[test]
    fn select_next_source_cycles_forward_and_wraps() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(
            "L1".into(), Protocol::Test, None,
            0.0, 0.0, 100, 100, 0,
            TextureMode::Fit, false, false,
        ));
        canvas.sources.push(Source::new(
            "L2".into(), Protocol::Test, None,
            100.0, 0.0, 100, 100, 1,
            TextureMode::Fit, false, false,
        ));
        let mut engine = test_engine(canvas);
        let uuids: Vec<String> = engine.cfg.canvas.sources.iter().map(|s| s.uuid.clone()).collect();

        engine.select_next_source();
        assert_eq!(engine.selected_layer_id.as_ref(), Some(&uuids[0]));

        engine.selected_layer_id = Some(uuids[0].clone());
        engine.select_next_source();
        assert_eq!(engine.selected_layer_id.as_ref(), Some(&uuids[1]));

        engine.select_next_source();
        assert_eq!(engine.selected_layer_id.as_ref(), Some(&uuids[0]));
    }

    #[test]
    fn select_previous_source_cycles_backward_and_wraps() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(
            "L1".into(), Protocol::Test, None,
            0.0, 0.0, 100, 100, 0,
            TextureMode::Fit, false, false,
        ));
        canvas.sources.push(Source::new(
            "L2".into(), Protocol::Test, None,
            100.0, 0.0, 100, 100, 1,
            TextureMode::Fit, false, false,
        ));
        let mut engine = test_engine(canvas);
        let uuids: Vec<String> = engine.cfg.canvas.sources.iter().map(|s| s.uuid.clone()).collect();

        engine.selected_layer_id = Some(uuids[0].clone());
        engine.select_previous_source();
        assert_eq!(engine.selected_layer_id.as_ref(), Some(&uuids[1]));

        engine.select_previous_source();
        assert_eq!(engine.selected_layer_id.as_ref(), Some(&uuids[0]));
    }

    #[test]
    fn nudge_selected_source_moves_by_delta() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(
            "L1".into(), Protocol::Test, None,
            10.0, 20.0, 100, 100, 0,
            TextureMode::Fit, false, false,
        ));
        let mut engine = test_engine(canvas);
        let uuid = engine.cfg.canvas.sources[0].uuid.clone();

        engine.selected_layer_id = Some(uuid.clone());
        engine.nudge_selected_source(3.0, -5.0);
        let source = &engine.cfg.canvas.sources[0];
        assert_eq!(source.x, 13.0);
        assert_eq!(source.y, 15.0);
        assert!(engine.dirty);
    }

    #[test]
    fn nudge_selected_source_ignores_when_expanded() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        canvas.sources.push(Source::new(
            "L1".into(), Protocol::Test, None,
            10.0, 20.0, 100, 100, 0,
            TextureMode::Fit, false, false,
        ));
        let mut engine = test_engine(canvas);
        let uuid = engine.cfg.canvas.sources[0].uuid.clone();

        engine.selected_layer_id = Some(uuid.clone());
        engine.expand_source(uuid.clone());
        engine.nudge_selected_source(3.0, -5.0);
        let source = &engine.cfg.canvas.sources[0];
        assert_eq!(source.x, 10.0);
        assert_eq!(source.y, 20.0);
    }

    fn should_auto_save(engine: &Engine, now: Instant) -> bool {
        engine.dirty
            && engine.project_path.is_some()
            && now.duration_since(engine.last_saved_at) >= AUTO_SAVE_INTERVAL
    }

    #[test]
    fn auto_save_triggers_after_interval_when_dirty() {
        let mut engine = test_engine(crate::config::Canvas::default());
        engine.project_path = Some(std::path::PathBuf::from("/tmp/test.multiviewer"));
        engine.dirty = true;
        engine.last_saved_at = Instant::now() - AUTO_SAVE_INTERVAL - Duration::from_secs(1);
        assert!(should_auto_save(&engine, Instant::now()));
    }

    #[test]
    fn auto_save_does_not_trigger_when_clean() {
        let mut engine = test_engine(crate::config::Canvas::default());
        engine.project_path = Some(std::path::PathBuf::from("/tmp/test.multiviewer"));
        engine.dirty = false;
        engine.last_saved_at = Instant::now() - AUTO_SAVE_INTERVAL - Duration::from_secs(1);
        assert!(!should_auto_save(&engine, Instant::now()));
    }

    #[test]
    fn auto_save_does_not_trigger_without_project_path() {
        let mut engine = test_engine(crate::config::Canvas::default());
        engine.project_path = None;
        engine.dirty = true;
        engine.last_saved_at = Instant::now() - AUTO_SAVE_INTERVAL - Duration::from_secs(1);
        assert!(!should_auto_save(&engine, Instant::now()));
    }

    /// A cfg output entry without its runtime object (project saved before the
    /// protocol was wired up) must be recreated on enable, or the side-panel
    /// checkbox silently flips back off while cfg says enabled.
    #[test]
    fn enabling_output_with_missing_runtime_recreates_it() {
        let mut engine = test_engine(crate::config::Canvas::default());
        let output = crate::config::Output::new(
            "NDI".to_string(),
            Protocol::Ndi,
            false,
            OutputConfig::Ndi(NdiOutputConfig {
                sender_name: "Test".to_string(),
            }),
        );
        let id = output.uuid.clone();
        engine.cfg.canvas.outputs.push(output);

        engine.set_output_enabled(&id, true);

        assert!(engine.cfg.canvas.outputs.iter().any(|o| o.uuid == id && o.enabled));
        let registered = engine.output_registry.get(&id).expect("runtime object recreated");
        assert!(registered.enabled());
    }

    /// An edit on one quad persists to every quad bound to the same
    /// (protocol, source_ref) — and to no quad bound to a different source or
    /// protocol, even when the reference string is identical.
    #[test]
    fn sync_source_config_updates_quads_sharing_the_key() {
        use crate::sources::decklink::VideoConnection;
        use crate::sources::NdiSourceConfig;
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            sources: vec![],
            outputs: Vec::new(),
            ..Default::default()
        };
        let ndi = |bw| SourceConfig::Ndi(NdiSourceConfig {
            bandwidth: bw,
            color_format: grafton_ndi::ReceiverColorFormat::UYVY_RGBA,
        });
        let highest = grafton_ndi::ReceiverBandwidth::Highest;
        let lowest = grafton_ndi::ReceiverBandwidth::Lowest;

        let mut quad_a = Source::new(
            "Quad A".into(), Protocol::Ndi, Some("Cam (1)".into()),
            0.0, 0.0, 960, 540, 0, TextureMode::Fit, false, false,
        );
        quad_a.config = ndi(highest);
        let mut quad_b = Source::new(
            "Quad B".into(), Protocol::Ndi, Some("Cam (1)".into()),
            960.0, 0.0, 960, 540, 1, TextureMode::Fit, false, false,
        );
        quad_b.config = ndi(highest);
        let mut quad_c = Source::new(
            "Quad C".into(), Protocol::Ndi, Some("Other Cam".into()),
            0.0, 540.0, 960, 540, 2, TextureMode::Fit, false, false,
        );
        quad_c.config = ndi(highest);
        let mut quad_d = Source::new(
            "Quad D".into(), Protocol::Decklink, Some("Cam (1)".into()),
            960.0, 540.0, 960, 540, 3, TextureMode::Fit, false, false,
        );
        quad_d.config = SourceConfig::Decklink(DecklinkSourceConfig {
            connection: VideoConnection::Hdmi,
        });
        canvas.sources.extend([quad_a, quad_b, quad_c, quad_d]);
        let mut engine = test_engine(canvas);
        let uuid_a = engine.cfg.canvas.sources[0].uuid.clone();

        engine.sync_source_config(&uuid_a, ndi(lowest));

        let [a, b, c, d] = &engine.cfg.canvas.sources[..] else {
            panic!("expected four sources")
        };
        // Same (protocol, source_ref): both quads show the edit.
        let (SourceConfig::Ndi(ca), SourceConfig::Ndi(cb)) = (&a.config, &b.config) else {
            panic!("expected Ndi configs")
        };
        assert_eq!(ca.bandwidth, lowest);
        assert_eq!(cb.bandwidth, lowest);
        // Same protocol, different source_ref: untouched.
        let SourceConfig::Ndi(cc) = &c.config else {
            panic!("expected Ndi config")
        };
        assert_eq!(cc.bandwidth, highest);
        // Same reference string under another protocol is a different key.
        let SourceConfig::Decklink(cd) = &d.config else {
            panic!("expected Decklink config")
        };
        assert_eq!(cd.connection, VideoConnection::Hdmi);
        assert!(engine.dirty);
    }
}
