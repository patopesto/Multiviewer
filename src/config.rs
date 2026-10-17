use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sources::{Protocol, SourceConfig, SourceRef, OutputConfig};

/// Schema version of the project file. Bump on every breaking change.
pub const CONFIG_VERSION: u32 = 1;

/// Assumed when a project file has no `version` key. Pinned to the first
/// released schema, not to `CONFIG_VERSION`, or a missing version would be
/// read as the newest schema after a bump.
const LEGACY_VERSION: u32 = 1;

fn config_version() -> u32 {
    return LEGACY_VERSION;
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "config_version")]
    pub version: u32,
    pub canvas: Canvas,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            canvas: Canvas::default(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config IO error: {e}"),
            ConfigError::Json(e) => write!(f, "config JSON error: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io(e)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self {
        ConfigError::Json(e)
    }
}

impl Config {
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let s = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&s)?)
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let s = serde_json::to_string_pretty(self)?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

// @docs protocol="Canvas" direction=input
#[derive(Serialize, Deserialize, Clone)]
pub struct Canvas {
    // @docs label="Width"
    /// Canvas width in pixels.
    pub width: u32,
    // @docs label="Height"
    /// Canvas height in pixels.
    pub height: u32,
    pub label: LabelConfig,
    pub border: BorderConfig,
    // @docs hidden
    pub sources: Vec<Source>,
    // @docs hidden
    pub outputs: Vec<Output>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            label: LabelConfig::default(),
            border: BorderConfig::default(),
            sources: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

pub type SourceId = String;

pub type OutputId = String;

// @docs protocol="Source" direction=input
#[derive(Serialize, Deserialize, Clone)]
pub struct Source {
    // @docs hidden
    pub uuid: SourceId,
    // @docs label="Name" default=""
    /// Name of the source; shown on its label and in the source list.
    pub name: String,
    // @docs hidden
    pub protocol: Protocol,
    // @docs hidden
    pub source_ref: Option<SourceRef>,
    // @docs label="X" default=""
    /// Horizontal position on the canvas, in pixels.
    pub x: f32,
    // @docs label="Y" default=""
    /// Vertical position on the canvas, in pixels.
    pub y: f32,
    // @docs label="Width" default=""
    /// Placement width on the canvas, in pixels.
    pub width: u32,
    // @docs label="Height" default=""
    /// Placement height on the canvas, in pixels.
    pub height: u32,
    // @docs label="Order" default=""
    /// Stacking order; higher values draw on top.
    pub z: i32,
    // @docs label="Mode" default="Fit"
    /// How the frame fills the rectangle.
    pub mode: TextureMode,
    // @docs label="Flip H"
    /// Mirror the source horizontally.
    pub flip_h: bool,
    // @docs label="Flip V"
    /// Mirror the source vertically.
    pub flip_v: bool,
    // @docs label="Label"
    /// Whether this source's label is drawn, overriding the global setting.
    pub label_visibility: SourceLabelVisibility,
    // @docs label="Borders"
    /// Whether this source's border is drawn, overriding the global setting.
    pub border_visibility: SourceBorderVisibility,
    // @docs hidden
    pub config: SourceConfig,
}

impl Source {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        protocol: Protocol,
        source_ref: Option<SourceRef>,
        x: f32,
        y: f32,
        width: u32,
        height: u32,
        z: i32,
        mode: TextureMode,
        flip_h: bool,
        flip_v: bool,
    ) -> Self {
        Self {
            uuid: uuid::Uuid::new_v4().to_string(),
            name,
            protocol,
            source_ref,
            x,
            y,
            width,
            height,
            z,
            mode,
            flip_h,
            flip_v,
            label_visibility: SourceLabelVisibility::default(),
            border_visibility: SourceBorderVisibility::default(),
            config: SourceConfig::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Output {
    pub uuid: OutputId,
    pub name: String,
    pub protocol: Protocol,
    pub enabled: bool,
    pub config: OutputConfig,
}

impl Output {
    pub fn new(
        name: String,
        protocol: Protocol,
        enabled: bool,
        config: OutputConfig,
    ) -> Self {
        Self {
            uuid: uuid::Uuid::new_v4().to_string(),
            name,
            protocol,
            enabled,
            config,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextureMode {
    Fit,
    Fill,
    Stretch,
}

impl TextureMode {
    pub fn label(&self) -> &'static str {
        match self {
            TextureMode::Fit => "Fit",
            TextureMode::Fill => "Fill",
            TextureMode::Stretch => "Stretch",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct LabelConfig {
    // @docs label="Label visibility"
    /// Whether source labels are drawn.
    pub visibility: LabelVisibility,
    // @docs label="Label position"
    /// Where the label sits inside its source rectangle.
    pub position: LabelPosition,
    // @docs label="Label size"
    /// Label text height in pixels.
    pub size: f32,
    // @docs label="Label text color"
    /// Label text colour, as RGBA.
    pub text_color: [u8; 4],
    // @docs label="Label background"
    /// Label background colour, as RGBA.
    pub background_color: [u8; 4],
}

impl Default for LabelConfig {
    fn default() -> Self {
        Self {
            visibility: LabelVisibility::Hide,
            position: LabelPosition::TopLeft,
            size: 24.0,
            text_color: [255, 255, 255, 255],
            background_color: [0, 0, 0, 180],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LabelVisibility {
    #[default]
    Show,
    Hide,
}

impl LabelVisibility {
    pub fn label(&self) -> &'static str {
        match self {
            LabelVisibility::Show => "Show",
            LabelVisibility::Hide => "Hide",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LabelPosition {
    #[default]
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl LabelPosition {
    pub fn label(&self) -> &'static str {
        match self {
            LabelPosition::TopLeft => "Top Left",
            LabelPosition::TopCenter => "Top Center",
            LabelPosition::TopRight => "Top Right",
            LabelPosition::CenterLeft => "Center Left",
            LabelPosition::Center => "Center",
            LabelPosition::CenterRight => "Center Right",
            LabelPosition::BottomLeft => "Bottom Left",
            LabelPosition::BottomCenter => "Bottom Center",
            LabelPosition::BottomRight => "Bottom Right",
        }
    }

    pub fn all() -> &'static [LabelPosition] {
        &[
            LabelPosition::TopLeft,
            LabelPosition::TopCenter,
            LabelPosition::TopRight,
            LabelPosition::CenterLeft,
            LabelPosition::Center,
            LabelPosition::CenterRight,
            LabelPosition::BottomLeft,
            LabelPosition::BottomCenter,
            LabelPosition::BottomRight,
        ]
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SourceLabelVisibility {
    #[default]
    Inherit,
    Hide,
    Show,
}

impl SourceLabelVisibility {
    pub fn label(&self) -> &'static str {
        match self {
            SourceLabelVisibility::Inherit => "Inherit",
            SourceLabelVisibility::Show => "Always show",
            SourceLabelVisibility::Hide => "Always hide",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct BorderConfig {
    // @docs label="Border visibility"
    /// Whether source borders are drawn.
    pub visibility: BorderVisibility,
    // @docs label="Border color"
    /// Border colour, as RGBA.
    pub color: [u8; 4],
    // @docs label="Border width"
    /// Border thickness in pixels.
    pub width: f32,
}

impl Default for BorderConfig {
    fn default() -> Self {
        Self {
            visibility: BorderVisibility::default(),
            color: [180, 180, 180, 255],
            width: 1.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BorderVisibility {
    #[default]
    Show,
    Hide,
}

impl BorderVisibility {
    pub fn label(&self) -> &'static str {
        match self {
            BorderVisibility::Show => "Show",
            BorderVisibility::Hide => "Hide",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SourceBorderVisibility {
    #[default]
    Inherit,
    Hide,
    Show,
}

impl SourceBorderVisibility {
    pub fn label(&self) -> &'static str {
        match self {
            SourceBorderVisibility::Inherit => "Inherit",
            SourceBorderVisibility::Show => "Always show",
            SourceBorderVisibility::Hide => "Always hide",
        }
    }
}


#[cfg(test)]
#[path = "config_test.rs"]
mod tests;
