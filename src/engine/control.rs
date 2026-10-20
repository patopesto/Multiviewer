use super::Engine;
use super::project::ProjectError;
use crate::config::Source;
use crate::control::{
    Command, ControlStatus, Selector, is_loopback_bind, normalize_prefix,
};
use crate::sources::SourceKey;

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
                self.selected_source_id = Some(uuid);
            }
            Command::RemoveSource(selector) => {
                let uuid = self.resolve_source(&selector).ok_or(ControlError::UnknownSource)?;
                self.remove_source(&uuid);
            }
            Command::SelectNext => self.select_source(1),
            Command::SelectPrevious => self.select_source(-1),
            Command::SelectSource(selector) => {
                let uuid = self.resolve_source(&selector).ok_or(ControlError::UnknownSource)?;
                self.selected_source_id = Some(uuid);
            }
            Command::ExpandSource(selector) => {
                let uuid = self.resolve_source(&selector).ok_or(ControlError::UnknownSource)?;
                self.expand_source(uuid);
            }
            Command::ExitExpanded => self.clear_expanded_source(),
            Command::Nudge { dx, dy } => {
                self.nudge_selected_source(finite(dx)?, finite(dy)?);
            }
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
            Command::SetCanvasSize { width, height } => {
                self.cfg.canvas.width = width.clamp(100, 7680);
                self.cfg.canvas.height = height.clamp(100, 7680);
                self.dirty = true;
            }
            Command::Zoom { factor } => {
                let factor = finite(factor)?;
                if factor <= 0.0 {
                    return Err(ControlError::InvalidArgument);
                }
                let rect = self.canvas_rect;
                self.zoom_view(&rect, factor);
            }
            Command::RecenterView => {
                let rect = self.canvas_rect;
                self.recenter_view(&rect);
            }
            Command::ToggleUi => self.ui_visible = !self.ui_visible,
            Command::SetOutputEnabled { output, enabled } => {
                let uuid = self.resolve_output(&output).ok_or(ControlError::UnknownOutput)?;
                self.set_output_enabled(&uuid, enabled);
            }
            Command::RestartOutput(output) => {
                let uuid = self.resolve_output(&output).ok_or(ControlError::UnknownOutput)?;
                self.restart_output(&uuid);
            }
            Command::SaveProject => self.save_project()?,
        }
        return Ok(());
    }

    /// Resolve a control selector to a source placement uuid.
    pub fn resolve_source(&self, selector: &Selector) -> Option<String> {
        return match selector {
            Selector::Index(index) => index
                .checked_sub(1)
                .and_then(|index| self.cfg.canvas.sources.get(index))
                .map(|source| source.uuid.clone()),
            Selector::Key(key) => self
                .cfg
                .canvas
                .sources
                .iter()
                .find(|source| &source.name == key)
                .or_else(|| {
                    self.cfg
                        .canvas
                        .sources
                        .iter()
                        .find(|source| &source.uuid == key)
                })
                .map(|source| source.uuid.clone()),
        };
    }

    /// Resolve a control selector to an output uuid.
    pub fn resolve_output(&self, selector: &Selector) -> Option<String> {
        return match selector {
            Selector::Index(index) => index
                .checked_sub(1)
                .and_then(|index| self.cfg.canvas.outputs.get(index))
                .map(|output| output.uuid.clone()),
            Selector::Key(key) => self
                .cfg
                .canvas
                .outputs
                .iter()
                .find(|output| &output.name == key)
                .or_else(|| {
                    self.cfg
                        .canvas
                        .outputs
                        .iter()
                        .find(|output| &output.uuid == key)
                })
                .map(|output| output.uuid.clone()),
        };
    }

    fn update_source(
        &mut self,
        selector: &Selector,
        update: impl FnOnce(&mut Source),
    ) -> Result<(), ControlError> {
        let uuid = self.resolve_source(selector).ok_or(ControlError::UnknownSource)?;
        let source = self
            .cfg
            .canvas
            .sources
            .iter_mut()
            .find(|source| source.uuid == uuid)
            .ok_or(ControlError::UnknownSource)?;
        update(source);
        self.dirty = true;
        return Ok(());
    }

    /// Whether a placement's runtime source is live.
    pub fn is_source_connected(&self, source: &Source) -> bool {
        return source
            .source_ref
            .as_ref()
            .map(|source_ref| {
                self.registry
                    .contains(&SourceKey::new(source.protocol.clone(), source_ref.clone()))
            })
            .unwrap_or(false);
    }

    /// Restart the listeners to match the current project configuration.
    pub fn restart_control(&mut self) {
        if let Ok(prefix) = normalize_prefix(&self.cfg.control.osc.prefix)
            && prefix != self.cfg.control.osc.prefix
        {
            self.cfg.control.osc.prefix = prefix;
            self.dirty = true;
        }
        let config = self.cfg.control.clone();
        self.control.apply(&config);
    }

    /// Allow a non-loopback bind for this session and restart the listeners.
    pub fn confirm_remote_control(&mut self) {
        let config = self.cfg.control.clone();
        self.control.confirm_remote(&config);
    }

    pub fn control_status_label(&self) -> String {
        return match self.control.status() {
            ControlStatus::Stopped => "Stopped".to_string(),
            ControlStatus::Listening(addr) => format!("Listening on {addr}"),
            ControlStatus::Error(e) => format!("Error: {e}"),
        };
    }

    pub fn control_needs_remote_confirmation(&self) -> bool {
        let osc = &self.cfg.control.osc;
        return osc.enabled && !self.control.remote_confirmed() && !is_loopback_bind(&osc.bind);
    }

    /// Control stays responsive while the window is unfocused.
    pub fn control_enabled(&self) -> bool {
        return self.control.is_enabled();
    }
}

fn finite(value: f32) -> Result<f32, ControlError> {
    if value.is_finite() {
        return Ok(value);
    }
    return Err(ControlError::InvalidArgument);
}
