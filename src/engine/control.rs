use super::Engine;
use super::project::ProjectError;
use crate::config::Source;
use crate::control::{
    Command, Selector, feedback_values, normalize_prefix,
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
            Command::ClearSelection => self.selected_source_id = None,
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
            Command::SetAutoSaveInterval(seconds) => {
                self.cfg.auto_save_interval = seconds;
                self.dirty = true;
            }
        }
        return Ok(());
    }

    /// Resolve a control selector to a source placement uuid.
    pub fn resolve_source(&self, selector: &Selector) -> Option<String> {
        return resolve(
            &self.cfg.canvas.sources,
            selector,
            |source| source.name.as_str(),
            |source| source.uuid.as_str(),
        );
    }

    /// Resolve a control selector to an output uuid.
    pub fn resolve_output(&self, selector: &Selector) -> Option<String> {
        return resolve(
            &self.cfg.canvas.outputs,
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
        let prefix = normalize_prefix(&self.cfg.control.osc.prefix).unwrap_or_default();
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
