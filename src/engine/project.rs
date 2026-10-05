use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::Engine;
use crate::config::{Config, ConfigError, Source, TextureMode, CONFIG_VERSION};
use crate::session::{Session, SessionError};
use crate::sources::{OutputRegistry, Protocol, SourceConfig, SourceRegistry};
use crate::sources::DecklinkDiscovery;
use crate::sources::NdiDiscovery;
#[cfg(target_os = "macos")]
use crate::sources::AvFoundationDiscovery;
#[cfg(target_os = "macos")]
use crate::sources::ScreenCaptureKitDiscovery;
#[cfg(target_os = "macos")]
use crate::sources::SyphonDiscovery;
#[cfg(target_os = "windows")]
use crate::sources::DirectShowDiscovery;
#[cfg(target_os = "windows")]
use crate::sources::MediaFoundationDiscovery;
#[cfg(target_os = "windows")]
use crate::sources::SpoutDiscovery;
#[cfg(target_os = "windows")]
use crate::sources::WindowsCaptureDiscovery;

use super::interaction::DragState;
use super::{SnapGuides, ViewState};

pub const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum ProjectError {
    Config(ConfigError),
    Session(SessionError),
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

impl From<ConfigError> for ProjectError {
    fn from(e: ConfigError) -> Self {
        ProjectError::Config(e)
    }
}

impl From<SessionError> for ProjectError {
    fn from(e: SessionError) -> Self {
        ProjectError::Session(e)
    }
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

        // Start ScreenCaptureKit display discovery on macOS
        #[cfg(target_os = "macos")]
        let screencapturekit = Some(ScreenCaptureKitDiscovery::start());

        // Start Spout sender discovery on Windows
        #[cfg(target_os = "windows")]
        let spout = Some(SpoutDiscovery::start());

        // Start Media Foundation device discovery on Windows
        #[cfg(target_os = "windows")]
        let mediafoundation = Some(MediaFoundationDiscovery::start());

        // Start DirectShow device discovery on Windows
        #[cfg(target_os = "windows")]
        let directshow = Some(DirectShowDiscovery::start());

        // Start Windows Graphics Capture discovery on Windows
        #[cfg(target_os = "windows")]
        let windowscapture = Some(WindowsCaptureDiscovery::start());

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
            #[cfg(target_os = "macos")]
            screencapturekit,
            #[cfg(target_os = "windows")]
            spout,
            #[cfg(target_os = "windows")]
            mediafoundation,
            #[cfg(target_os = "windows")]
            directshow,
            #[cfg(target_os = "windows")]
            windowscapture,
            comp: None,
            device: None,
            queue: None,
            project_path: None,
            dirty: false,
            last_saved_at: Instant::now(),
            selected_source_id: None,
            expanded_source_id: None,
            drag_state: DragState::None,
            snap_guides: SnapGuides::default(),
            view: ViewState::new(),
            load_warnings: Vec::new(),
        };

        engine.rebuild_from_config();
        return engine;
    }

    pub(super) fn rebuild_from_config(&mut self) {
        self.registry.clear();
        self.output_registry.clear();
        self.comp = None;
        self.load_warnings.clear();

        self.warn_unavailable_protocols();
        self.restore_test_sources();
        self.seed_demo_layout();
        self.load_outputs();
    }

    /// A project from a newer build is loaded best-effort; unknown fields are
    /// dropped on parse, so clamp the version we write back to what we understand.
    pub(super) fn warn_if_newer_version(&mut self) {
        if self.cfg.version <= CONFIG_VERSION {
            return;
        }
        self.load_warnings.push(format!(
            "Project was saved by a newer version ({} > {}); some settings may not load",
            self.cfg.version, CONFIG_VERSION
        ));
        self.cfg.version = CONFIG_VERSION;
    }

    /// Report protocols this platform cannot run. Their entries stay in the
    /// config untouched (raw JSON preserved) and are written back on save.
    fn warn_unavailable_protocols(&mut self) {
        let unavailable: BTreeSet<&str> = self
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
    }

    /// Restore Test sources for all Test sources in the loaded config.
    /// Each Test source gets a fresh dedicated test source.
    fn restore_test_sources(&mut self) {
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
    }

    /// Seed demo layout if nothing was loaded
    fn seed_demo_layout(&mut self) {
        if !self.cfg.canvas.sources.is_empty() {
            return;
        }
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

    fn load_outputs(&mut self) {
        for output in &mut self.cfg.canvas.outputs {
            if output.uuid.is_empty() {
                output.uuid = uuid::Uuid::new_v4().to_string();
                self.dirty = true;
            }
            self.output_registry.add(
                &output.protocol,
                output.uuid.clone(),
                output.name.clone(),
                &output.config,
                output.enabled,
            );
        }
    }

    pub fn open_project(&mut self, path: impl AsRef<std::path::Path>) -> Result<(), ProjectError> {
        let path = path.as_ref().to_path_buf();
        let cfg = Config::load_from(&path)?;
        self.cfg = cfg;
        self.dirty = false;
        self.rebuild_from_config();
        self.warn_if_newer_version();
        self.project_path = Some(path.clone());
        self.last_saved_at = Instant::now();
        self.dirty = false;
        let mut session = Session::load();
        session.record_recent(&path);
        session.save()?;
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
        let mut session = Session::load();
        session.record_recent(&path);
        session.save()?;
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
}
