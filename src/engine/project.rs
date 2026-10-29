use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::Engine;
use crate::config::{Config, ConfigError, Source, TextureMode, CONFIG_VERSION};
#[cfg(test)]
use crate::config::Canvas;
use crate::session::{Session, SessionError};
use crate::sources::{OutputRegistry, Protocol, SourceConfig, SourceRegistry};
use crate::sources::DecklinkDiscovery;
use crate::sources::NdiDiscovery;
use crate::sources::OmtDiscovery;
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
use crate::compositor::Rect;
use crate::control::ControlRegistry;

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
    /// An engine with no discovery services: `new_project` attaches the ones it
    /// starts, tests leave them all off.
    fn new(cfg: Config) -> Self {
        return Self {
            cfg,
            registry: SourceRegistry::new(),
            output_registry: OutputRegistry::new(),
            control: ControlRegistry::new(),
            ndi: None,
            decklink: None,
            omt: None,
            #[cfg(target_os = "macos")]
            syphon: None,
            #[cfg(target_os = "macos")]
            avfoundation: None,
            #[cfg(target_os = "macos")]
            screencapturekit: None,
            #[cfg(target_os = "windows")]
            spout: None,
            #[cfg(target_os = "windows")]
            mediafoundation: None,
            #[cfg(target_os = "windows")]
            directshow: None,
            #[cfg(target_os = "windows")]
            windowscapture: None,
            comp: None,
            project_path: None,
            dirty: false,
            last_saved_at: Instant::now(),
            last_discovery: Instant::now(),
            last_tick: Instant::now(),
            last_heartbeat: Instant::now(),
            worst_tick_gap: Duration::ZERO,
            selected_source_id: None,
            expanded_source_id: None,
            drag_state: DragState::None,
            snap_guides: SnapGuides::default(),
            view: ViewState::new(),
            canvas_rect: Rect::default(),
            ui_visible: true,
            load_warnings: Vec::new(),
        };
    }

    pub fn new_project() -> Self {
        let started = Instant::now();
        let mut engine = Self::new(Config::default());

        // NDI discovery has to be up before the project's sources are restored.
        engine.ndi = Some(NdiDiscovery::start());
        engine.decklink = Some(DecklinkDiscovery::start());
        engine.omt = Some(OmtDiscovery::start());
        #[cfg(target_os = "macos")]
        {
            engine.syphon = Some(SyphonDiscovery::start());
            engine.avfoundation = Some(AvFoundationDiscovery::start());
            engine.screencapturekit = Some(ScreenCaptureKitDiscovery::start());
        }
        #[cfg(target_os = "windows")]
        {
            engine.spout = Some(SpoutDiscovery::start());
            engine.mediafoundation = Some(MediaFoundationDiscovery::start());
            engine.directshow = Some(DirectShowDiscovery::start());
            engine.windowscapture = Some(WindowsCaptureDiscovery::start());
        }

        engine.rebuild_from_config();
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "engine created"
        );
        return engine;
    }

    pub(super) fn rebuild_from_config(&mut self) {
        let clear_started = Instant::now();
        self.registry.clear();
        let clear_sources = clear_started.elapsed();
        let clear_outputs_started = Instant::now();
        self.output_registry.clear();
        let clear_outputs = clear_outputs_started.elapsed();
        self.comp = None;
        self.load_warnings.clear();

        let load_started = Instant::now();
        self.warn_unavailable_protocols();
        self.restore_test_sources();
        self.seed_demo_layout();
        self.load_outputs();
        self.restart_control();
        let load = load_started.elapsed();

        // Every stage runs on the thread that opened the project, holding the
        // engine lock: a stage that blocks freezes the UI for its whole duration.
        tracing::info!(
            clear_sources_ms = clear_sources.as_millis() as u64,
            clear_outputs_ms = clear_outputs.as_millis() as u64,
            load_ms = load.as_millis() as u64,
            sources = self.cfg.canvas.sources.len(),
            outputs = self.cfg.canvas.outputs.len(),
            "project rebuilt"
        );
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
        let started = Instant::now();
        tracing::info!(path = %path.display(), "opening project");
        let cfg = match Config::load_from(&path) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::error!(path = %path.display(), "project open failed: {e}");
                return Err(e.into());
            }
        };
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
        tracing::info!(
            path = %path.display(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "project opened"
        );
        return Ok(());
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

    /// Whether the project is due for an auto-save at `now`.
    pub fn should_auto_save(&self, now: Instant) -> bool {
        return self.cfg.auto_save_interval > 0
            && self.dirty
            && self.project_path.is_some()
            && now.duration_since(self.last_saved_at)
                >= Duration::from_secs(self.cfg.auto_save_interval);
    }

    pub fn auto_save(&mut self) {
        if self.should_auto_save(Instant::now()) {
            tracing::debug!("Auto-saving project");
            if let Err(e) = self.save_project() {
                tracing::error!("Auto-save failed: {e}");
            }
        }
    }
}

#[cfg(test)]
impl Engine {
    /// Engine with no discovery services, for tests.
    pub fn test_instance(canvas: Canvas) -> Self {
        return Self::new(Config {
            canvas,
            ..Config::default()
        });
    }
}
