use serde::{Deserialize, Serialize};

use crate::config::{
    BorderConfig, LabelConfig, SourceBorderVisibility, SourceId, SourceLabelVisibility, TextureMode,
};
use crate::sources::{OutputConfig, Protocol, SourceConfig, SourceRef};

/// Editable placement fields that never touch the protocol runtime.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourcePlacementPatch {
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub width: u32,
    pub height: u32,
    pub z: i32,
    pub mode: TextureMode,
    pub flip_h: bool,
    pub flip_v: bool,
    pub label_visibility: SourceLabelVisibility,
    pub border_visibility: SourceBorderVisibility,
}

/// Which source or output a command targets.
///
/// `Id` is the stable identity used by the UI and HTTP clients. `Index` is the
/// 1-based list order used by OSC. `Key` matches a name first, then a uuid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Selector {
    Id(SourceId),
    Index(usize),
    Key(String),
}

/// Protocol-agnostic command vocabulary. Every control backend decodes its
/// wire format into one of these and hands it to `Engine::apply_command`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Command {
    AddSource,
    RemoveSource(Selector),
    SelectNext,
    SelectPrevious,
    SelectSource(Selector),
    ClearSelection,
    ExpandSource(Selector),
    ExitExpanded,
    Nudge {
        dx: f32,
        dy: f32,
    },
    SetSourceRect {
        source: Selector,
        x: f32,
        y: f32,
        width: u32,
        height: u32,
    },
    PatchSourcePlacement {
        source: Selector,
        patch: SourcePlacementPatch,
    },
    SetSourceConfig {
        source: Selector,
        config: SourceConfig,
    },
    SetSourceProtocol {
        source: Selector,
        protocol: Protocol,
    },
    SetSourceBinding {
        source: Selector,
        source_ref: Option<SourceRef>,
    },
    NewSource {
        source: Selector,
        protocol: Protocol,
    },
    MoveSource {
        from: usize,
        to: usize,
    },
    SetSourceX {
        source: Selector,
        value: f32,
    },
    SetSourceY {
        source: Selector,
        value: f32,
    },
    SetSourceWidth {
        source: Selector,
        value: u32,
    },
    SetSourceHeight {
        source: Selector,
        value: u32,
    },
    SetSourceSize {
        source: Selector,
        width: u32,
        height: u32,
    },
    SetSourceZ {
        source: Selector,
        value: i32,
    },
    SetSourceMode {
        source: Selector,
        mode: TextureMode,
    },
    SetSourceFlipH {
        source: Selector,
        value: bool,
    },
    SetSourceFlipV {
        source: Selector,
        value: bool,
    },
    SetCanvasSize {
        width: u32,
        height: u32,
    },
    PatchCanvas {
        width: u32,
        height: u32,
        label: LabelConfig,
        border: BorderConfig,
    },
    SetView {
        zoom: f32,
        pan: [f32; 2],
    },
    Zoom {
        factor: f32,
    },
    RecenterView,
    ToggleUi,
    SetOutputEnabled {
        output: Selector,
        enabled: bool,
    },
    AddOutput {
        protocol: Protocol,
        config: OutputConfig,
    },
    RemoveOutput(Selector),
    SetOutputConfig {
        output: Selector,
        config: OutputConfig,
    },
    RestartOutput(Selector),
    RefreshDiscovery {
        protocol: Protocol,
    },
    SaveProject,
    SetAutoSaveInterval(u64),
}
