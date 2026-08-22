use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::sources::{Protocol, SourceConfig, OutputConfig};

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub canvas: Canvas,
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

#[derive(Serialize, Deserialize, Clone)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub border_visibility: BorderVisibility,
    pub border_color: [u8; 4],
    pub border_width: f32,
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
            border_color: [180, 180, 180, 255],
            border_width: 1.0,
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
    #[serde(default)]
    pub config: SourceConfig,
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
            config: SourceConfig::default(),
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
    #[serde(default)]
    pub config: OutputConfig,
}

impl Output {
    pub fn new_v4(
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
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::sources::syphon::SyphonOutputConfig;
    use crate::sources::{NdiSourceConfig, NdiOutputConfig, DecklinkSourceConfig, DecklinkOutputConfig, TestSourceConfig};
    use crate::sources::decklink::{VideoConnection, VideoConnections};

    #[test]
    fn output_config_round_trips() {
        let output = Output::new_v4(
            "Multiviewer".to_string(),
            Protocol::Syphon,
            true,
            OutputConfig::Syphon(SyphonOutputConfig {
                server_name: "MyServer".to_string(),
            }),
        );
        let json = serde_json::to_string(&output).unwrap();
        let parsed: Output = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "Multiviewer");
        assert_eq!(parsed.protocol, Protocol::Syphon);
        assert!(parsed.enabled);
        match parsed.config {
            OutputConfig::Syphon(cfg) => assert_eq!(cfg.server_name, "MyServer"),
            _ => panic!("expected Syphon config"),
        }
    }

    #[test]
    fn output_config_deserializes_missing_config() {
        let json = r#"{"uuid":"abc","name":"Test","protocol":"Syphon","enabled":true}"#;
        let parsed: Output = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.protocol, Protocol::Syphon);
        match parsed.config {
            OutputConfig::Unknown => {} // Expected: missing config defaults to Unknown
            _ => panic!("expected Unknown config"),
        }
    }

    #[test]
    fn decklink_output_config_round_trips() {
        use multiviewer_decklink::DisplayMode;
        let output = Output::new_v4(
            "My DeckLink".into(),
            Protocol::Decklink,
            true,
            OutputConfig::Decklink(DecklinkOutputConfig {
                device_name: "DeckLink Mini Monitor".into(),
                display_mode: DisplayMode::Hd1080p6000,
                width: 1920,
                height: 1080,
            }),
        );
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"display_mode\":1215313456"));
        let parsed: Output = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed.config, OutputConfig::Decklink(_)));
        match parsed.config {
            OutputConfig::Decklink(c) => {
                assert_eq!(c.device_name, "DeckLink Mini Monitor");
                assert_eq!(c.display_mode, DisplayMode::Hd1080p6000);
            }
            _ => panic!("expected Decklink config"),
        }
    }

    #[test]
    fn ndi_output_config_round_trips() {
        let output = Output::new_v4(
            "My NDI".into(),
            Protocol::Ndi,
            true,
            OutputConfig::Ndi(NdiOutputConfig {
                sender_name: "Studio".into(),
            }),
        );
        let json = serde_json::to_string(&output).unwrap();
        let parsed: Output = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed.config, OutputConfig::Ndi(_)));
        match parsed.config {
            OutputConfig::Ndi(c) => assert_eq!(c.sender_name, "Studio"),
            _ => panic!("expected Ndi config"),
        }
    }

    #[test]
    fn test_source_config_round_trips() {
        let mut source = Source::new_v4(
            "Test".to_string(),
            Protocol::Test,
            Some("test-1".to_string()),
            0.0, 0.0, 1280, 720, 0,
            TextureMode::Fit,
            false, false,
        );
        source.config = SourceConfig::Test(TestSourceConfig { 
            width: 1920, 
            height: 1080,
            ..Default::default() 
        });
        let json = serde_json::to_string(&source).unwrap();
        let parsed: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.protocol, Protocol::Test);
        match parsed.config {
            SourceConfig::Test(c) => {
                assert_eq!(c.width, 1920);
                assert_eq!(c.height, 1080);
            }
            _ => panic!("expected Test source config"),
        }
    }

    #[test]
    fn ndi_source_config_round_trips() {
        let mut source = Source::new_v4(
            "NDI".to_string(),
            Protocol::Ndi,
            Some("ndi-1".to_string()),
            0.0, 0.0, 1920, 1080, 0,
            TextureMode::Fit,
            false, false,
        );
        source.config = SourceConfig::Ndi(NdiSourceConfig {
            bandwidth: grafton_ndi::ReceiverBandwidth::Highest,
            color_format: grafton_ndi::ReceiverColorFormat::UYVY_RGBA,
        });
        let json = serde_json::to_string(&source).unwrap();
        let parsed: Source = serde_json::from_str(&json).unwrap();
        match parsed.config {
            SourceConfig::Ndi(c) => {
                assert_eq!(c.bandwidth, grafton_ndi::ReceiverBandwidth::Highest);
                assert_eq!(c.color_format, grafton_ndi::ReceiverColorFormat::UYVY_RGBA);
            }
            _ => panic!("expected Ndi source config"),
        }
    }

    #[test]
    fn decklink_source_config_round_trips() {
        let mut source = Source::new_v4(
            "DeckLink".to_string(),
            Protocol::Decklink,
            Some("decklink-1".to_string()),
            0.0, 0.0, 1920, 1080, 0,
            TextureMode::Fit,
            false, false,
        );
        source.config = SourceConfig::Decklink(DecklinkSourceConfig {
            connection: VideoConnection::Hdmi,
            supported_connections: VideoConnections::EMPTY,
        });
        let json = serde_json::to_string(&source).unwrap();
        let parsed: Source = serde_json::from_str(&json).unwrap();
        match parsed.config {
            SourceConfig::Decklink(c) => {
                assert_eq!(c.connection, VideoConnection::Hdmi);
            }
            _ => panic!("expected Decklink source config"),
        }
    }

    #[test]
    fn config_round_trips_through_path() {
        let dir = std::env::temp_dir().join(format!("multiviewer-config-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.multiviewer");

        let mut cfg = Config::default();
        cfg.canvas.width = 1234;
        cfg.canvas.sources.push(Source::new_v4(
            "Test".to_string(),
            Protocol::Test,
            Some("test-1".to_string()),
            0.0, 0.0, 1280, 720, 0,
            TextureMode::Fit,
            false, false,
        ));
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.canvas.width, 1234);
        assert_eq!(loaded.canvas.sources.len(), 1);
        assert_eq!(loaded.canvas.sources[0].protocol, Protocol::Test);

        std::fs::remove_dir_all(&dir).ok();
    }
}
