use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub canvas: Canvas,
}

impl Config {
    pub fn load() -> Self {
        match std::fs::read_to_string(path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(path(), s) {
                    tracing::error!("config save failed: {e}");
                }
            }
            Err(e) => tracing::error!("config serialize failed: {e}"),
        }
    }
}

pub fn path() -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("multiviewer.json")
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub border_visibility: BorderVisibility,
    pub sources: Vec<Source>,
    #[serde(default)]
    pub outputs: Vec<Output>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            border_visibility: BorderVisibility::default(),
            sources: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Source {
    pub uuid: String,
    #[serde(default)]
    pub name: String,
    pub protocol: Protocol,
    pub source_id: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: u32,
    pub height: u32,
    pub z: i32,
    pub mode: TextureMode,
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
    #[serde(default)]
    pub border_visibility: SourceBorderVisibility,
}

impl Source {
    pub fn new_v4(
        name: String,
        protocol: Protocol,
        source_id: Option<String>,
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
            source_id,
            x,
            y,
            width,
            height,
            z,
            mode,
            flip_h,
            flip_v,
            border_visibility: SourceBorderVisibility::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Output {
    pub uuid: String,
    #[serde(default)]
    pub name: String,
    pub protocol: Protocol,
    pub enabled: bool,
}

impl Output {
    pub fn new_v4(
        name: String,
        protocol: Protocol,
        enabled: bool,
    ) -> Self {
        Self {
            uuid: uuid::Uuid::new_v4().to_string(),
            name,
            protocol,
            enabled,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Protocol {
    Test,
    Ndi,
    Syphon,
    Decklink,
}

impl Protocol {
    pub fn label(&self) -> &'static str {
        match self {
            Protocol::Test => "Test",
            Protocol::Ndi => "NDI",
            Protocol::Syphon => "Syphon",
            Protocol::Decklink => "DeckLink",
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
