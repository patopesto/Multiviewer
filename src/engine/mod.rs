use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::compositor::{Compositor, Rect};
use crate::config::{Config, SourceId};
use crate::control::ControlRegistry;
use crate::sources::{OutputRegistry, SourceRegistry};
use crate::sources::{DecklinkDiscovery, decklink_version};
use crate::sources::{NdiDiscovery, ndi_version};
use crate::sources::{OmtDiscovery, omt_version};
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

mod control;
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

#[derive(Clone, Copy, Default)]
pub struct RunTotals {
    pub ticks: u64,
    pub work_ms: f64,
    pub gpu_ms: f64,
    pub gpu_samples: u64,
    pub uploads: u64,
    pub upload_bytes: u64,
    pub upload_fallbacks: u64,
}

impl RunTotals {
    pub fn mean_work_ms(&self) -> f32 {
        if self.ticks == 0 {
            return 0.0;
        }
        return (self.work_ms / self.ticks as f64) as f32;
    }

    pub fn mean_gpu_ms(&self) -> Option<f32> {
        if self.gpu_samples == 0 {
            return None;
        }
        return Some((self.gpu_ms / self.gpu_samples as f64) as f32);
    }
}

pub struct Engine {
    pub cfg: Config,
    pub registry: SourceRegistry,
    pub output_registry: OutputRegistry,
    pub control: ControlRegistry,
    // Protocols
    pub ndi: Option<NdiDiscovery>,
    pub decklink: Option<DecklinkDiscovery>,
    pub omt: Option<OmtDiscovery>,
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
    pub project_path: Option<PathBuf>,
    pub dirty: bool,
    last_saved_at: Instant,
    last_discovery: Instant,
    last_tick: Instant,
    last_heartbeat: Instant,
    worst_tick_gap: Duration,
    pub selected_source_id: Option<SourceId>,
    pub expanded_source_id: Option<SourceId>,
    pub drag_state: DragState,
    pub snap_guides: SnapGuides,
    pub view: ViewState,
    /// Last canvas panel rectangle, used by control commands that zoom or recenter.
    pub canvas_rect: Rect,
    pub ui_visible: bool,
    pub load_warnings: Vec<String>,
    totals: RunTotals,
}

impl Engine {
    /// Protocol/driver versions of the linked third-party libraries, for display.
    pub fn vendor_versions() -> Vec<(&'static str, Option<String>)> {
        return Vec::from([
            ("NDI", ndi_version()),
            ("OMT", omt_version()),
            ("DeckLink", decklink_version()),
            #[cfg(target_os = "macos")]
            ("Syphon", syphon_version()),
            #[cfg(target_os = "windows")]
            ("Spout", spout_version()),
        ]);
    }
}
