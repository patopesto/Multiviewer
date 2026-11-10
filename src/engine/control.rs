use super::Engine;
use super::transform::{MAX_ZOOM, MIN_ZOOM};
use super::project::ProjectError;
use crate::config::{BorderVisibility, LabelVisibility, Source};
use crate::control::{
    Command, Selector, feedback_values, normalize_prefix,
};
use crate::sources::{
    ConfigApply, Protocol, RdpSourceConfig, SourceConfig, SourceKey, VncSourceConfig,
};

#[derive(Debug)]
pub enum ControlError {
    UnknownSource,
    UnknownOutput,
    InvalidArgument,
    Project(ProjectError),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::UnknownSource => write!(f, "unknown source"),
            ControlError::UnknownOutput => write!(f, "unknown output"),
            ControlError::InvalidArgument => write!(f, "invalid argument"),
            ControlError::Project(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ControlError {}

impl From<ProjectError> for ControlError {
    fn from(e: ProjectError) -> Self {
        return ControlError::Project(e);
    }
}

impl Engine {
    /// Apply one protocol-agnostic control command. The single dispatch point
    /// shared by every control backend.
    pub fn apply_command(&mut self, command: Command) -> Result<(), ControlError> {
        tracing::debug!(command = ?command, "control command");
        match command {
            Command::AddSource => {
                let uuid = self.add_source();
                self.state.selected_source_id = Some(uuid);
            }
            Command::RemoveSource(selector) => {
                let uuid = self.resolve_source(&selector).ok_or(ControlError::UnknownSource)?;
                self.remove_source(&uuid);
            }
            Command::SelectNext => self.select_source(1),
            Command::SelectPrevious => self.select_source(-1),
            Command::SelectSource(selector) => {
                let uuid = self.resolve_source(&selector).ok_or(ControlError::UnknownSource)?;
                self.state.selected_source_id = Some(uuid);
            }
            Command::ClearSelection => self.state.selected_source_id = None,
            Command::ExpandSource(selector) => {
                let uuid = self.resolve_source(&selector).ok_or(ControlError::UnknownSource)?;
                self.expand_source(uuid);
            }
            Command::ExitExpanded => self.clear_expanded_source(),
            Command::Nudge { source, dx, dy } => {
                let dx = finite(dx)?;
                let dy = finite(dy)?;
                if self.state.expanded_source_id.is_none() {
                    let selector = source.or_else(|| {
                        self.state
                            .selected_source_id
                            .clone()
                            .map(Selector::Id)
                    });
                    if let Some(selector) = selector {
                        self.update_source(&selector, |source| {
                            source.x += dx;
                            source.y += dy;
                        })?;
                    }
                }
            }
            Command::SetSourceRect {
                source,
                x,
                y,
                width,
                height,
            } => {
                let x = finite(x)?;
                let y = finite(y)?;
                self.update_source(&source, |source| {
                    source.x = x;
                    source.y = y;
                    source.width = width.max(1);
                    source.height = height.max(1);
                })?;
            }
            Command::PatchSourcePlacement { source, patch } => {
                let x = finite(patch.x)?;
                let y = finite(patch.y)?;
                self.update_source(&source, |source| {
                    source.name = patch.name;
                    source.x = x;
                    source.y = y;
                    source.width = patch.width.max(1);
                    source.height = patch.height.max(1);
                    source.z = patch.z;
                    source.mode = patch.mode;
                    source.flip_h = patch.flip_h;
                    source.flip_v = patch.flip_v;
                    source.label_visibility = patch.label_visibility;
                    source.border_visibility = patch.border_visibility;
                })?;
            }
            Command::SetSourceConfig { source, config } => {
                let uuid = self.resolve_source(&source).ok_or(ControlError::UnknownSource)?;
                let (protocol, source_ref) = self
                    .state
                    .cfg
                    .canvas
                    .sources
                    .iter()
                    .find(|placement| placement.uuid == uuid)
                    .map(|placement| (placement.protocol.clone(), placement.source_ref.clone()))
                    .ok_or(ControlError::UnknownSource)?;
                if !config.matches(&protocol) {
                    return Err(ControlError::InvalidArgument);
                }
                let apply = match &source_ref {
                    Some(source_ref) => {
                        let key = SourceKey::new(protocol.clone(), source_ref.clone());
                        self.registry
                            .get_mut(&key)
                            .map(|kind| kind.apply_config(config.clone()))
                            .unwrap_or(ConfigApply::Persist)
                    }
                    None => ConfigApply::Persist,
                };
                self.sync_source_config(&uuid, config);
                if apply == ConfigApply::Restart
                    && let Some(source_ref) = source_ref
                {
                    self.registry
                        .restart(&SourceKey::new(protocol, source_ref));
                }
            }
            Command::SetSourceProtocol { source, protocol } => {
                let uuid = self.resolve_source(&source).ok_or(ControlError::UnknownSource)?;
                let placement = self
                    .state
                    .cfg
                    .canvas
                    .sources
                    .iter_mut()
                    .find(|placement| placement.uuid == uuid)
                    .ok_or(ControlError::UnknownSource)?;
                placement.protocol = protocol;
                placement.source_ref = None;
                placement.config = SourceConfig::for_protocol(&placement.protocol);
                self.state.dirty = true;
            }
            Command::SetSourceBinding { source, source_ref } => {
                let uuid = self.resolve_source(&source).ok_or(ControlError::UnknownSource)?;
                let protocol = self
                    .state
                    .cfg
                    .canvas
                    .sources
                    .iter()
                    .find(|placement| placement.uuid == uuid)
                    .map(|placement| placement.protocol.clone())
                    .ok_or(ControlError::UnknownSource)?;
                if let Some(placement) = self
                    .state
                    .cfg
                    .canvas
                    .sources
                    .iter_mut()
                    .find(|placement| placement.uuid == uuid)
                {
                    placement.source_ref = source_ref.clone();
                }
                self.state.dirty = true;
                if let Some(name) = source_ref {
                    self.connect(protocol, &name);
                }
            }
            Command::NewSource { source, protocol } => {
                let uuid = self.resolve_source(&source).ok_or(ControlError::UnknownSource)?;
                let (source_ref, config) = match protocol {
                    Protocol::Test => (
                        self.registry.add_test(None).source_ref,
                        Some(SourceConfig::for_protocol(&Protocol::Test)),
                    ),
                    Protocol::Vnc => {
                        let config = VncSourceConfig::default();
                        let key = self.registry.add_vnc(config.clone());
                        (key.source_ref, Some(SourceConfig::Vnc(config)))
                    }
                    Protocol::Rdp => {
                        let config = RdpSourceConfig::default();
                        let key = self.registry.add_rdp(config.clone());
                        (key.source_ref, Some(SourceConfig::Rdp(config)))
                    }
                    _ => return Err(ControlError::InvalidArgument),
                };
                let placement = self
                    .state
                    .cfg
                    .canvas
                    .sources
                    .iter_mut()
                    .find(|placement| placement.uuid == uuid)
                    .ok_or(ControlError::UnknownSource)?;
                placement.source_ref = Some(source_ref);
                if let Some(config) = config {
                    placement.config = config;
                }
                self.state.dirty = true;
            }
            Command::MoveSource { from, to } => self.move_source(from, to),
            Command::SetSourceX { source, value } => {
                let value = finite(value)?;
                self.update_source(&source, |source| source.x = value)?;
            }
            Command::SetSourceY { source, value } => {
                let value = finite(value)?;
                self.update_source(&source, |source| source.y = value)?;
            }
            Command::SetSourceWidth { source, value } => {
                self.update_source(&source, |source| source.width = value.max(1))?;
            }
            Command::SetSourceHeight { source, value } => {
                self.update_source(&source, |source| source.height = value.max(1))?;
            }
            Command::SetSourceSize {
                source,
                width,
                height,
            } => {
                self.update_source(&source, |source| {
                    source.width = width.max(1);
                    source.height = height.max(1);
                })?;
            }
            Command::SetSourceZ { source, value } => {
                self.update_source(&source, |source| source.z = value)?;
            }
            Command::SetSourceMode { source, mode } => {
                self.update_source(&source, |source| source.mode = mode)?;
            }
            Command::SetSourceFlipH { source, value } => {
                self.update_source(&source, |source| source.flip_h = value)?;
            }
            Command::SetSourceFlipV { source, value } => {
                self.update_source(&source, |source| source.flip_v = value)?;
            }
            Command::SetSourceName { source, value } => {
                self.update_source(&source, |source| source.name = value)?;
            }
            Command::SetSourceLabelVisibility { source, value } => {
                self.update_source(&source, |source| source.label_visibility = value)?;
            }
            Command::SetSourceBorderVisibility { source, value } => {
                self.update_source(&source, |source| source.border_visibility = value)?;
            }
            Command::SetCanvasSize { width, height } => {
                self.state.cfg.canvas.width = width.clamp(100, 7680);
                self.state.cfg.canvas.height = height.clamp(100, 7680);
                self.state.dirty = true;
            }
            Command::SetCanvasLabelVisibility { enabled } => {
                self.state.cfg.canvas.label.visibility = if enabled {
                    LabelVisibility::Show
                } else {
                    LabelVisibility::Hide
                };
                self.state.dirty = true;
            }
            Command::SetCanvasBorderVisibility { enabled } => {
                self.state.cfg.canvas.border.visibility = if enabled {
                    BorderVisibility::Show
                } else {
                    BorderVisibility::Hide
                };
                self.state.dirty = true;
            }
            Command::PatchCanvas {
                width,
                height,
                label,
                border,
            } => {
                self.state.cfg.canvas.width = width.clamp(100, 7680);
                self.state.cfg.canvas.height = height.clamp(100, 7680);
                self.state.cfg.canvas.label = label;
                self.state.cfg.canvas.border = border;
                self.state.dirty = true;
            }
            Command::SetView { zoom, pan } => {
                self.state.view.zoom = finite(zoom)?.clamp(MIN_ZOOM, MAX_ZOOM);
                self.state.view.pan = [finite(pan[0])?, finite(pan[1])?];
            }
            Command::Zoom { factor } => {
                let factor = finite(factor)?;
                if factor <= 0.0 {
                    return Err(ControlError::InvalidArgument);
                }
                let rect = self.canvas_rect;
                self.state.view.zoom(&self.state.cfg.canvas, &rect, factor);
            }
            Command::RecenterView => {
                let rect = self.canvas_rect;
                self.state.view.recenter(&self.state.cfg.canvas, &rect);
            }
            Command::ToggleUi => self.state.ui_visible = !self.state.ui_visible,
            Command::SetOutputEnabled { output, enabled } => {
                let uuid = self.resolve_output(&output).ok_or(ControlError::UnknownOutput)?;
                self.set_output_enabled(&uuid, enabled);
            }
            Command::AddOutput { protocol, config } => {
                let name = format!("{} Output", protocol.label());
                self.add_output(protocol, name, config);
            }
            Command::RemoveOutput(output) => {
                let uuid = self.resolve_output(&output).ok_or(ControlError::UnknownOutput)?;
                self.remove_output(&uuid);
            }
            Command::SetOutputConfig { output, config } => {
                let uuid = self.resolve_output(&output).ok_or(ControlError::UnknownOutput)?;
                let placement = self
                    .state
                    .cfg
                    .canvas
                    .outputs
                    .iter_mut()
                    .find(|placement| placement.uuid == uuid)
                    .ok_or(ControlError::UnknownOutput)?;
                placement.config = config;
                self.state.dirty = true;
                self.restart_output(&uuid);
            }
            Command::RestartOutput(output) => {
                let uuid = self.resolve_output(&output).ok_or(ControlError::UnknownOutput)?;
                self.restart_output(&uuid);
            }
            Command::RefreshDiscovery { protocol } => {
                self.refresh_discovery(&protocol);
            }
            Command::SaveProject => self.save_project()?,
            Command::SetAutoSaveInterval(seconds) => {
                self.state.cfg.auto_save_interval = seconds;
                self.state.dirty = true;
            }
        }
        self.state.revision = self.state.revision.wrapping_add(1);
        return Ok(());
    }

    /// Resolve a control selector to a source placement uuid.
    pub fn resolve_source(&self, selector: &Selector) -> Option<String> {
        return resolve(
            &self.state.cfg.canvas.sources,
            selector,
            |source| source.name.as_str(),
            |source| source.uuid.as_str(),
        );
    }

    /// Resolve a control selector to an output uuid.
    pub fn resolve_output(&self, selector: &Selector) -> Option<String> {
        return resolve(
            &self.state.cfg.canvas.outputs,
            selector,
            |output| output.name.as_str(),
            |output| output.uuid.as_str(),
        );
    }

    fn update_source(
        &mut self,
        selector: &Selector,
        update: impl FnOnce(&mut Source),
    ) -> Result<(), ControlError> {
        let uuid = self.resolve_source(selector).ok_or(ControlError::UnknownSource)?;
        let source = self
            .state.cfg
            .canvas
            .sources
            .iter_mut()
            .find(|source| source.uuid == uuid)
            .ok_or(ControlError::UnknownSource)?;
        update(source);
        self.state.dirty = true;
        return Ok(());
    }

    /// Restart the listeners to match the current project configuration.
    pub fn restart_control(&mut self) {
        if let Ok(prefix) = normalize_prefix(&self.state.cfg.control.osc.prefix)
            && prefix != self.state.cfg.control.osc.prefix
        {
            self.state.cfg.control.osc.prefix = prefix;
            self.state.dirty = true;
        }
        let config = self.state.cfg.control.clone();
        self.control.apply(&config);
    }

    pub fn refresh_discovery(&self, protocol: &Protocol) {
        match protocol {
            Protocol::Decklink => {
                if let Some(discovery) = &self.decklink {
                    discovery.request_refresh();
                }
            }
            #[cfg(target_os = "macos")]
            Protocol::ScreenCaptureKit => {
                if let Some(discovery) = &self.screencapturekit {
                    discovery.request_refresh();
                }
            }
            _ => {}
        }
    }

    /// Whether a placement's runtime source is live.
    pub fn is_source_connected(&self, source: &Source) -> bool {
        return source
            .source_ref
            .as_ref()
            .is_some_and(|source_ref| {
                self.registry
                    .is_live(&SourceKey::new(source.protocol.clone(), source_ref.clone()))
            });
    }

    /// Publish OSC feedback for any state that changed since the last frame.
    pub fn publish_control_feedback(&mut self) {
        if !self.control.feedback_enabled() {
            return;
        }
        let prefix = normalize_prefix(&self.state.cfg.control.osc.prefix).unwrap_or_default();
        let values = feedback_values(self, &prefix);
        self.control.publish_feedback(values);
    }
}

/// Resolve a selector against a canvas list: 1-based index, then name, then uuid.
fn resolve<T>(
    items: &[T],
    selector: &Selector,
    name: fn(&T) -> &str,
    uuid: fn(&T) -> &str,
) -> Option<String> {
    return match selector {
        Selector::Id(id) => items
            .iter()
            .any(|item| uuid(item) == id.as_str())
            .then(|| id.clone()),
        Selector::Index(index) => index
            .checked_sub(1)
            .and_then(|index| items.get(index))
            .map(|item| uuid(item).to_string()),
        Selector::Key(key) => items
            .iter()
            .find(|item| name(item) == key)
            .or_else(|| items.iter().find(|item| uuid(item) == key))
            .map(|item| uuid(item).to_string()),
    };
}

fn finite(value: f32) -> Result<f32, ControlError> {
    if value.is_finite() {
        return Ok(value);
    }
    return Err(ControlError::InvalidArgument);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BorderVisibility, Canvas, LabelVisibility, Source, SourceBorderVisibility,
        SourceLabelVisibility, TextureMode,
    };
    use crate::engine::state::EngineState;
    use crate::engine::SourcePlacementPatch;
    use crate::sources::{
        NdiOutputConfig, OutputConfig, Protocol, SourceConfig, SourceKey, TestSourceConfig,
    };

    fn engine_with_source() -> (Engine, String) {
        let source = Source::new(
            "Source".into(),
            Protocol::Test,
            None,
            0.0,
            0.0,
            640,
            360,
            0,
            TextureMode::Fit,
            false,
            false,
        );
        let id = source.uuid.clone();
        let mut canvas = Canvas::default();
        canvas.sources.push(source);
        return (Engine::test_instance(canvas), id);
    }

    #[test]
    fn placement_and_selection_are_published_in_the_snapshot() {
        let (mut engine, id) = engine_with_source();

        engine
            .apply_command(Command::SetSourceRect {
                source: Selector::Id(id.clone()),
                x: 10.0,
                y: 20.0,
                width: 800,
                height: 450,
            })
            .unwrap();
        engine
            .apply_command(Command::SelectSource(Selector::Id(id.clone())))
            .unwrap();

        let snapshot = engine.snapshot();
        let source = snapshot
            .state
            .cfg
            .canvas
            .sources
            .iter()
            .find(|source| source.uuid == id)
            .unwrap();
        assert_eq!((source.x, source.y), (10.0, 20.0));
        assert_eq!((source.width, source.height), (800, 450));
        assert_eq!(
            snapshot.state.selected_source_id.as_deref(),
            Some(id.as_str())
        );
        assert_eq!(snapshot.state.revision, 2);
    }

    #[test]
    fn view_and_expand_commands_update_the_snapshot() {
        let (mut engine, id) = engine_with_source();
        engine
            .apply_command(Command::ExpandSource(Selector::Id(id.clone())))
            .unwrap();
        engine
            .apply_command(Command::SetView {
                zoom: 2.0,
                pan: [4.0, 6.0],
            })
            .unwrap();

        let snapshot = engine.snapshot();
        assert_eq!(
            snapshot.state.expanded_source_id.as_deref(),
            Some(id.as_str())
        );
        assert_eq!(snapshot.state.view.zoom, 2.0);
        assert_eq!(snapshot.state.view.pan, [4.0, 6.0]);
    }

    #[test]
    fn remove_source_command_prunes_the_placement() {
        let (mut engine, id) = engine_with_source();
        engine
            .apply_command(Command::RemoveSource(Selector::Id(id)))
            .unwrap();

        let snapshot = engine.snapshot();
        assert!(snapshot.state.cfg.canvas.sources.is_empty());
        assert!(snapshot.state.selected_source_id.is_none());
        assert_eq!(snapshot.state.revision, 1);
    }

    #[test]
    fn missing_placement_is_rejected_without_bumping_the_revision() {
        let (mut engine, _id) = engine_with_source();
        let result = engine.apply_command(Command::SetSourceRect {
            source: Selector::Id("missing".into()),
            x: 0.0,
            y: 0.0,
            width: 10,
            height: 10,
        });
        assert!(result.is_err());
        assert_eq!(engine.snapshot().state.revision, 0);
    }

    #[test]
    fn command_and_snapshot_round_trip_through_json() {
        let (engine, id) = engine_with_source();
        let command = Command::SetSourceRect {
            source: Selector::Id(id.clone()),
            x: 1.0,
            y: 2.0,
            width: 640,
            height: 360,
        };
        let json = serde_json::to_string(&command).unwrap();
        let decoded: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, command);

        let json = serde_json::to_string(&engine.snapshot().state).unwrap();
        let decoded: EngineState = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.revision, 0);
        assert_eq!(decoded.cfg.canvas.sources.len(), 1);
    }

    #[test]
    fn placement_patch_updates_only_the_placement() {
        let (mut engine, id) = engine_with_source();
        engine
            .apply_command(Command::PatchSourcePlacement {
                source: Selector::Id(id.clone()),
                patch: SourcePlacementPatch {
                    name: "Renamed".into(),
                    x: 5.0,
                    y: 6.0,
                    width: 100,
                    height: 50,
                    z: 3,
                    mode: TextureMode::Fill,
                    flip_h: true,
                    flip_v: false,
                    label_visibility: SourceLabelVisibility::Hide,
                    border_visibility: SourceBorderVisibility::Show,
                },
            })
            .unwrap();

        let source = engine
            .state
            .cfg
            .canvas
            .sources
            .iter()
            .find(|source| source.uuid == id)
            .unwrap();
        assert_eq!(source.name, "Renamed");
        assert_eq!((source.x, source.y), (5.0, 6.0));
        assert_eq!((source.width, source.height), (100, 50));
        assert_eq!(source.z, 3);
        assert_eq!(source.mode, TextureMode::Fill);
        assert!(source.flip_h);
        assert!(!source.flip_v);
        assert_eq!(source.label_visibility, SourceLabelVisibility::Hide);
        assert_eq!(source.border_visibility, SourceBorderVisibility::Show);
    }

    #[test]
    fn narrow_placement_commands_update_only_their_field() {
        let (mut engine, id) = engine_with_source();
        engine
            .apply_command(Command::SetSourceName {
                source: Selector::Id(id.clone()),
                value: "Cam A".into(),
            })
            .unwrap();
        engine
            .apply_command(Command::SetSourceLabelVisibility {
                source: Selector::Id(id.clone()),
                value: SourceLabelVisibility::Show,
            })
            .unwrap();
        engine
            .apply_command(Command::SetSourceBorderVisibility {
                source: Selector::Id(id.clone()),
                value: SourceBorderVisibility::Hide,
            })
            .unwrap();

        let source = engine
            .state
            .cfg
            .canvas
            .sources
            .iter()
            .find(|source| source.uuid == id)
            .unwrap();
        assert_eq!(source.name, "Cam A");
        assert_eq!(source.label_visibility, SourceLabelVisibility::Show);
        assert_eq!(source.border_visibility, SourceBorderVisibility::Hide);
        assert_eq!((source.x, source.y), (0.0, 0.0));
    }

    #[test]
    fn canvas_visibility_commands_toggle_label_and_border() {
        let (mut engine, _id) = engine_with_source();
        engine
            .apply_command(Command::SetCanvasLabelVisibility { enabled: true })
            .unwrap();
        engine
            .apply_command(Command::SetCanvasBorderVisibility { enabled: false })
            .unwrap();

        assert_eq!(engine.state.cfg.canvas.label.visibility, LabelVisibility::Show);
        assert_eq!(engine.state.cfg.canvas.border.visibility, BorderVisibility::Hide);
    }

    #[test]
    fn nudge_targets_the_selection_or_an_explicit_source() {
        let (mut engine, id) = engine_with_source();
        let position = |engine: &Engine| {
            let source = engine
                .state
                .cfg
                .canvas
                .sources
                .iter()
                .find(|source| source.uuid == id)
                .unwrap();
            (source.x, source.y)
        };

        engine
            .apply_command(Command::Nudge {
                source: None,
                dx: 10.0,
                dy: 10.0,
            })
            .unwrap();
        assert_eq!(position(&engine), (0.0, 0.0));

        engine.state.selected_source_id = Some(id.clone());
        engine
            .apply_command(Command::Nudge {
                source: None,
                dx: 5.0,
                dy: -2.0,
            })
            .unwrap();
        assert_eq!(position(&engine), (5.0, -2.0));

        engine.state.selected_source_id = None;
        engine
            .apply_command(Command::Nudge {
                source: Some(Selector::Id(id.clone())),
                dx: -1.0,
                dy: 1.0,
            })
            .unwrap();
        assert_eq!(position(&engine), (4.0, -1.0));

        engine
            .apply_command(Command::ExpandSource(Selector::Id(id.clone())))
            .unwrap();
        engine
            .apply_command(Command::Nudge {
                source: Some(Selector::Index(1)),
                dx: 100.0,
                dy: 100.0,
            })
            .unwrap();
        assert_eq!(position(&engine), (4.0, -1.0));
    }

    #[test]
    fn canvas_patch_updates_canvas_settings() {
        let (mut engine, _id) = engine_with_source();
        let mut label = engine.state.cfg.canvas.label;
        label.size = 42.0;
        let border = engine.state.cfg.canvas.border;
        engine
            .apply_command(Command::PatchCanvas {
                width: 1280,
                height: 720,
                label,
                border,
            })
            .unwrap();

        let snapshot = engine.snapshot();
        assert_eq!((snapshot.state.cfg.canvas.width, snapshot.state.cfg.canvas.height), (1280, 720));
        assert_eq!(snapshot.state.cfg.canvas.label.size, 42.0);
    }

    #[test]
    fn new_source_creates_a_runtime_and_publishes_it() {
        let (mut engine, id) = engine_with_source();
        engine
            .apply_command(Command::NewSource {
                source: Selector::Id(id.clone()),
                protocol: Protocol::Test,
            })
            .unwrap();

        let snapshot = engine.snapshot();
        let source = snapshot
            .state
            .cfg
            .canvas
            .sources
            .iter()
            .find(|source| source.uuid == id)
            .unwrap();
        assert!(source.source_ref.is_some());
        assert_eq!(snapshot.runtime.sources.len(), 1);
        assert!(snapshot.runtime.discovery(&Protocol::Test).is_some());
    }

    #[test]
    fn source_protocol_change_clears_the_binding() {
        let (mut engine, id) = engine_with_source();
        engine.state.cfg.canvas.sources[0].source_ref = Some("ref".into());
        engine
            .apply_command(Command::SetSourceProtocol {
                source: Selector::Id(id.clone()),
                protocol: Protocol::Ndi,
            })
            .unwrap();

        let source = &engine.state.cfg.canvas.sources[0];
        assert_eq!(source.protocol, Protocol::Ndi);
        assert!(source.source_ref.is_none());
    }

    #[test]
    fn source_binding_sets_the_source_ref() {
        let (mut engine, id) = engine_with_source();
        engine
            .apply_command(Command::SetSourceBinding {
                source: Selector::Id(id.clone()),
                source_ref: Some("camera".into()),
            })
            .unwrap();

        let source = &engine.state.cfg.canvas.sources[0];
        assert_eq!(source.source_ref.as_deref(), Some("camera"));
    }

    #[test]
    fn move_source_reorders_the_list() {
        let (mut engine, first) = engine_with_source();
        engine.apply_command(Command::AddSource).unwrap();
        let second = engine.state.selected_source_id.clone().unwrap();

        engine
            .apply_command(Command::MoveSource { from: 1, to: 0 })
            .unwrap();

        let order: Vec<_> = engine
            .state
            .cfg
            .canvas
            .sources
            .iter()
            .map(|source| source.uuid.clone())
            .collect();
        assert_eq!(order, vec![second, first]);
    }

    #[test]
    fn source_config_command_updates_the_placement() {
        let (mut engine, id) = engine_with_source();
        let mut config = TestSourceConfig::default();
        config.width = 320;
        engine
            .apply_command(Command::SetSourceConfig {
                source: Selector::Id(id.clone()),
                config: SourceConfig::Test(config),
            })
            .unwrap();

        let source = &engine.state.cfg.canvas.sources[0];
        assert!(matches!(&source.config, SourceConfig::Test(config) if config.width == 320));
        assert!(engine.state.dirty);
    }

    #[test]
    fn new_commands_round_trip_through_json() {
        let (_engine, id) = engine_with_source();
        let commands = vec![
            Command::PatchSourcePlacement {
                source: Selector::Id(id.clone()),
                patch: SourcePlacementPatch {
                    name: "Name".into(),
                    x: 1.0,
                    y: 2.0,
                    width: 640,
                    height: 360,
                    z: 0,
                    mode: TextureMode::Fit,
                    flip_h: false,
                    flip_v: false,
                    label_visibility: SourceLabelVisibility::Inherit,
                    border_visibility: SourceBorderVisibility::Inherit,
                },
            },
            Command::SetSourceConfig {
                source: Selector::Id(id.clone()),
                config: SourceConfig::Test(TestSourceConfig::default()),
            },
            Command::SetSourceProtocol {
                source: Selector::Id(id.clone()),
                protocol: Protocol::Ndi,
            },
            Command::SetSourceBinding {
                source: Selector::Id(id),
                source_ref: Some("ref".into()),
            },
            Command::MoveSource { from: 0, to: 1 },
            Command::RefreshDiscovery {
                protocol: Protocol::Ndi,
            },
            Command::AddOutput {
                protocol: Protocol::Ndi,
                config: OutputConfig::Ndi(NdiOutputConfig::default()),
            },
        ];
        for command in commands {
            let json = serde_json::to_string(&command).unwrap();
            let decoded: Command = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, command);
        }
    }

    #[test]
    fn zoom_and_recenter_use_the_panel_rect() {
        let (mut engine, _id) = engine_with_source();
        engine.canvas_rect = crate::compositor::Rect {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 1000.0,
        };

        engine
            .apply_command(Command::Zoom { factor: 2.0 })
            .unwrap();
        assert!(engine.state.view.zoom > 1.0);
        assert!(engine.state.view.pan[0].is_finite());
        assert!(engine.state.view.pan[1].is_finite());

        engine.apply_command(Command::RecenterView).unwrap();
        assert!(engine.state.view.zoom.is_finite() && engine.state.view.zoom > 0.0);
    }

    #[test]
    fn protocol_change_resets_the_config() {
        let (mut engine, id) = engine_with_source();
        engine.state.cfg.canvas.sources[0].config = SourceConfig::for_protocol(&Protocol::Ndi);

        engine
            .apply_command(Command::SetSourceProtocol {
                source: Selector::Id(id.clone()),
                protocol: Protocol::Decklink,
            })
            .unwrap();

        let source = &engine.state.cfg.canvas.sources[0];
        assert_eq!(source.protocol, Protocol::Decklink);
        assert!(matches!(source.config, SourceConfig::Decklink(_)));
    }

    #[test]
    fn set_source_config_rejects_a_foreign_protocol() {
        let (mut engine, id) = engine_with_source();

        let result = engine.apply_command(Command::SetSourceConfig {
            source: Selector::Id(id),
            config: SourceConfig::for_protocol(&Protocol::Ndi),
        });

        assert!(result.is_err());
        assert!(matches!(
            engine.state.cfg.canvas.sources[0].config,
            SourceConfig::Unknown(_)
        ));
        assert_eq!(engine.state.revision, 0);
    }

    #[test]
    fn new_source_sets_the_placement_config() {
        let (mut engine, id) = engine_with_source();
        engine
            .apply_command(Command::NewSource {
                source: Selector::Id(id.clone()),
                protocol: Protocol::Test,
            })
            .unwrap();

        let source = engine
            .state
            .cfg
            .canvas
            .sources
            .iter()
            .find(|source| source.uuid == id)
            .unwrap();
        assert!(source.source_ref.is_some());
        assert!(matches!(source.config, SourceConfig::Test(_)));
    }

    #[test]
    fn source_binding_adopts_the_live_runtime_config() {
        let (mut engine, first_id) = engine_with_source();
        engine
            .apply_command(Command::NewSource {
                source: Selector::Id(first_id.clone()),
                protocol: Protocol::Test,
            })
            .unwrap();
        engine.apply_command(Command::AddSource).unwrap();
        let second_id = engine.state.selected_source_id.clone().unwrap();

        let first_ref = engine
            .state
            .cfg
            .canvas
            .sources
            .iter()
            .find(|source| source.uuid == first_id)
            .and_then(|source| source.source_ref.clone())
            .unwrap();
        let key = SourceKey::new(Protocol::Test, first_ref.clone());
        let custom = TestSourceConfig {
            width: 320,
            ..TestSourceConfig::default()
        };
        engine.registry.get_mut(&key).unwrap().config = SourceConfig::Test(custom.clone());

        engine
            .apply_command(Command::SetSourceBinding {
                source: Selector::Id(second_id.clone()),
                source_ref: Some(first_ref),
            })
            .unwrap();

        let second = engine
            .state
            .cfg
            .canvas
            .sources
            .iter()
            .find(|source| source.uuid == second_id)
            .unwrap();
        assert!(matches!(&second.config, SourceConfig::Test(config) if config.width == 320));
    }
}
