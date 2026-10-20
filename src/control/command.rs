use serde::{Deserialize, Serialize};

use crate::config::TextureMode;

/// Which source or output a command targets. `Index` is 1-based list order;
/// `Key` matches a name first and a uuid second.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Selector {
    Index(usize),
    Key(String),
}

/// Protocol-agnostic control vocabulary. Every control backend decodes its
/// wire format into one of these and hands it to `Engine::apply_command`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    AddSource,
    RemoveSource(Selector),
    SelectNext,
    SelectPrevious,
    SelectSource(Selector),
    ExpandSource(Selector),
    ExitExpanded,
    Nudge { dx: f32, dy: f32 },
    SetSourceX { source: Selector, value: f32 },
    SetSourceY { source: Selector, value: f32 },
    SetSourceWidth { source: Selector, value: u32 },
    SetSourceHeight { source: Selector, value: u32 },
    SetSourceSize { source: Selector, width: u32, height: u32 },
    SetSourceZ { source: Selector, value: i32 },
    SetSourceMode { source: Selector, mode: TextureMode },
    SetSourceFlipH { source: Selector, value: bool },
    SetSourceFlipV { source: Selector, value: bool },
    SetCanvasSize { width: u32, height: u32 },
    Zoom { factor: f32 },
    RecenterView,
    ToggleUi,
    SetOutputEnabled { output: Selector, enabled: bool },
    RestartOutput(Selector),
    SaveProject,
}
