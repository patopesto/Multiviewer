use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::compositor::Compositor;
use crate::config::{Config, SourceId};
use crate::sources::{OutputRegistry, SourceRegistry};
use crate::sources::{DecklinkDiscovery, decklink_version};
use crate::sources::{NdiDiscovery, ndi_version};
#[cfg(target_os = "macos")]
use crate::sources::{SyphonDiscovery, syphon_version};
#[cfg(target_os = "macos")]
use crate::sources::AvFoundationDiscovery;
#[cfg(target_os = "macos")]
use crate::sources::ScreenCaptureKitDiscovery;
#[cfg(target_os = "windows")]
use crate::sources::{SpoutDiscovery, spout_version};
#[cfg(target_os = "windows")]
use crate::sources::MediaFoundationDiscovery;
#[cfg(target_os = "windows")]
use crate::sources::DirectShowDiscovery;
#[cfg(target_os = "windows")]
use crate::sources::WindowsCaptureDiscovery;

mod interaction;
mod outputs;
mod project;
mod render;
mod sources;
#[cfg(test)]
mod tests;

pub use interaction::DragState;

#[derive(Clone, Copy, Debug, Default)]
pub struct ViewState {
    pub zoom: f32,
    pub pan: egui::Vec2,
}

impl ViewState {
    pub fn new() -> Self {
        Self {
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SnapGuides {
    pub x: Option<f32>,
    pub y: Option<f32>,
}

pub struct Engine {
    pub cfg: Config,
    pub registry: SourceRegistry,
    pub output_registry: OutputRegistry,
    // Protocols
    pub ndi: Option<NdiDiscovery>,
    pub decklink: Option<DecklinkDiscovery>,
    #[cfg(target_os = "macos")]
    pub syphon: Option<SyphonDiscovery>,
    #[cfg(target_os = "macos")]
    pub avfoundation: Option<AvFoundationDiscovery>,
    #[cfg(target_os = "macos")]
    pub screencapturekit: Option<ScreenCaptureKitDiscovery>,
    #[cfg(target_os = "windows")]
    pub spout: Option<SpoutDiscovery>,
    #[cfg(target_os = "windows")]
    pub mediafoundation: Option<MediaFoundationDiscovery>,
    #[cfg(target_os = "windows")]
    pub directshow: Option<DirectShowDiscovery>,
    #[cfg(target_os = "windows")]
    pub windowscapture: Option<WindowsCaptureDiscovery>,
    // Other
    comp: Option<Compositor>,
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    pub project_path: Option<PathBuf>,
    pub dirty: bool,
    last_saved_at: Instant,
    pub selected_source_id: Option<SourceId>,
    pub expanded_source_id: Option<SourceId>,
    pub drag_state: DragState,
    pub snap_guides: SnapGuides,
    pub view: ViewState,
    pub load_warnings: Vec<String>,
}

impl Engine {
    /// Protocol/driver versions of the linked third-party libraries, for display.
    pub fn vendor_versions() -> Vec<(&'static str, Option<String>)> {
        return Vec::from([
            ("NDI", ndi_version()),
            ("DeckLink", decklink_version()),
            #[cfg(target_os = "macos")]
            ("Syphon", syphon_version()),
            #[cfg(target_os = "windows")]
            ("Spout", spout_version()),
        ]);
    }
}
