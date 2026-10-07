use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use windows_capture::capture::{
    CaptureControl, Context, GraphicsCaptureApiError, GraphicsCaptureApiHandler,
};
use windows_capture::frame::Frame as WgcFrame;
use windows_capture::graphics_capture_api::{Error as CaptureApiError, InternalCaptureControl};
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use super::super::{CpuFrame, Frame, FramePool, PixelFormat, SourceRef, SourceStats, VideoSource};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureCursor {
    #[default]
    Default,
    Show,
    Hide,
}

impl CaptureCursor {
    pub fn label(&self) -> &'static str {
        return match self {
            Self::Default => "Default",
            Self::Show => "Show",
            Self::Hide => "Hide",
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureBorder {
    #[default]
    Default,
    Show,
    Hide,
}

impl CaptureBorder {
    pub fn label(&self) -> &'static str {
        return match self {
            Self::Default => "Default",
            Self::Show => "Show",
            Self::Hide => "Hide",
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSecondaryWindows {
    #[default]
    Default,
    Include,
    Exclude,
}

impl CaptureSecondaryWindows {
    pub fn label(&self) -> &'static str {
        return match self {
            Self::Default => "Default",
            Self::Include => "Include",
            Self::Exclude => "Exclude",
        };
    }
}

/// Session tunables applied when the capture starts; changing one restarts the source.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct WindowsCaptureSettings {
    pub cursor: CaptureCursor,
    pub border: CaptureBorder,
    pub secondary_windows: CaptureSecondaryWindows,
    pub max_fps: u32, // 0 = uncapped
}

impl WindowsCaptureSettings {
    pub fn is_default(&self) -> bool {
        return self == &Self::default();
    }
}

/// Capture target plus session tunables, tagged on the wire as `kind`; older flat configs do not parse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WindowsCaptureSourceConfig {
    Display {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        device_name: String,
        settings: WindowsCaptureSettings,
    },
    Window {
        hwnd: u64,
        process_name: String,
        title: String,
        settings: WindowsCaptureSettings,
    },
}

impl WindowsCaptureSourceConfig {
    pub fn settings(&self) -> &WindowsCaptureSettings {
        return match self {
            Self::Display { settings, .. } => settings,
            Self::Window { settings, .. } => settings,
        };
    }

    pub fn settings_mut(&mut self) -> &mut WindowsCaptureSettings {
        return match self {
            Self::Display { settings, .. } => settings,
            Self::Window { settings, .. } => settings,
        };
    }

    pub fn is_window(&self) -> bool {
        return matches!(self, Self::Window { .. });
    }
}

impl Default for WindowsCaptureSourceConfig {
    /// Placeholder for a source whose target has not been picked yet.
    fn default() -> Self {
        return Self::Display {
            device_name: String::new(),
            settings: WindowsCaptureSettings::default(),
        };
    }
}

struct CaptureHandler {
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    seq: u64,
    pool: FramePool,
}

impl GraphicsCaptureApiHandler for CaptureHandler {
    type Flags = (Arc<Mutex<Option<Frame>>>, Arc<Mutex<SourceStats>>);
    type Error = BoxError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let (slot, stats) = ctx.flags;
        return Ok(Self {
            slot,
            stats,
            seq: 0,
            pool: FramePool::new(),
        });
    }

    #[instrument(name = "windowscapture_frame", level = "debug", skip_all)]
    fn on_frame_arrived(
        &mut self,
        frame: &mut WgcFrame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let w = frame.width();
        let h = frame.height();

        let t0 = Instant::now();
        let mut buffer = frame.buffer().map_err(|e| -> BoxError { Box::new(e) })?;
        let row_pitch = buffer.row_pitch();
        let raw = buffer.as_raw_buffer();
        let len = raw.len();

        let mut buf = self.pool.take(len);
        buf.copy_from_slice(raw);
        let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;

        let seq = self.seq;
        self.seq += 1;

        {
            let mut s = self.stats.lock().unwrap();
            s.record_frame(w, h, PixelFormat::Bgra8.label(), 0.0);
            s.record_copy_time(copy_ms);
        }

        let frame = Frame::Cpu(CpuFrame {
            data: Arc::new(buf),
            w,
            h,
            fmt: PixelFormat::Bgra8,
            pitch: row_pitch,
            seq,
        });
        let old = self.slot.lock().unwrap().replace(frame);
        self.pool.give(old);

        return Ok(());
    }
}

enum CaptureItem {
    Monitor(Monitor),
    Window(Window),
}

fn resolve_target(config: &WindowsCaptureSourceConfig) -> Result<CaptureItem, String> {
    return match config {
        WindowsCaptureSourceConfig::Display { device_name, .. } => {
            let name = device_name.as_str();
            let monitors = Monitor::enumerate()
                .map_err(|e| format!("could not enumerate monitors: {e}"))?;
            let monitor = monitors
                .into_iter()
                .find(|m| m.device_name().unwrap_or_default() == name)
                .ok_or_else(|| format!("monitor {name} not found"))?;
            Ok(CaptureItem::Monitor(monitor))
        }
        WindowsCaptureSourceConfig::Window {
            hwnd,
            process_name,
            title,
            ..
        } => {
            let window = Window::from_raw_hwnd((*hwnd as usize) as *mut c_void);
            if window.is_valid() {
                let w_title = window.title().unwrap_or_default();
                let w_process = window.process_name().unwrap_or_default();
                if w_process == process_name.as_str() && w_title == title.as_str() {
                    return Ok(CaptureItem::Window(window));
                }
            }
            // hwnd went stale or was reused: re-find by process + title.
            let windows = Window::enumerate()
                .map_err(|e| format!("could not enumerate windows: {e}"))?;
            let found = windows.into_iter().find(|w| {
                let w_title = w.title().unwrap_or_default();
                let w_process = w.process_name().unwrap_or_default();
                return w_process == process_name.as_str() && w_title == title.as_str();
            });
            let Some(window) = found else {
                return Err(format!(
                    "window {process_name} / {title} not found"
                ));
            };
            return Ok(CaptureItem::Window(window));
        }
    };
}

/// Start capture, retrying with defaults once if the OS build rejects a
/// requested setting; `None` means the source stays frozen.
fn start_capture<T>(
    source_ref: &str,
    item: T,
    settings: &WindowsCaptureSettings,
    is_window: bool,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
) -> Option<CaptureControl<CaptureHandler, BoxError>>
where
    T: TryInto<GraphicsCaptureItemType> + Send + 'static + Copy,
{
    match try_start(item, settings, is_window, slot.clone(), stats.clone()) {
        Ok(control) => return Some(control),
        Err(e) if !settings.is_default() && is_unsupported_settings(&e) => {
            tracing::warn!(
                source = %source_ref,
                "Requested Windows Capture settings unsupported on this build ({e}); retrying with defaults"
            );
        }
        Err(e) => {
            tracing::error!(source = %source_ref, "Failed to start Windows capture: {e}");
            return None;
        }
    }
    return match try_start(item, &WindowsCaptureSettings::default(), is_window, slot, stats) {
        Ok(control) => Some(control),
        Err(e) => {
            tracing::error!(source = %source_ref, "Failed to start Windows capture with defaults: {e}");
            None
        }
    };
}

fn try_start<T>(
    item: T,
    settings: &WindowsCaptureSettings,
    is_window: bool,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
) -> Result<CaptureControl<CaptureHandler, BoxError>, GraphicsCaptureApiError<BoxError>>
where
    T: TryInto<GraphicsCaptureItemType> + Send + 'static,
{
    let cursor = match settings.cursor {
        CaptureCursor::Default => CursorCaptureSettings::Default,
        CaptureCursor::Show => CursorCaptureSettings::WithCursor,
        CaptureCursor::Hide => CursorCaptureSettings::WithoutCursor,
    };
    let border = match settings.border {
        CaptureBorder::Default => DrawBorderSettings::Default,
        CaptureBorder::Show => DrawBorderSettings::WithBorder,
        CaptureBorder::Hide => DrawBorderSettings::WithoutBorder,
    };
    // Secondary windows only apply to window targets.
    let secondary = if !is_window {
        SecondaryWindowSettings::Default
    } else {
        match settings.secondary_windows {
            CaptureSecondaryWindows::Default => SecondaryWindowSettings::Default,
            CaptureSecondaryWindows::Include => SecondaryWindowSettings::Include,
            CaptureSecondaryWindows::Exclude => SecondaryWindowSettings::Exclude,
        }
    };

    let min_interval = if settings.max_fps == 0 {
        MinimumUpdateIntervalSettings::Default
    } else {
        MinimumUpdateIntervalSettings::Custom(Duration::from_secs_f64(1.0 / settings.max_fps as f64))
    };

    let crate_settings = Settings::new(
        item,
        cursor,
        border,
        secondary,
        min_interval,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        (slot, stats),
    );

    return <CaptureHandler as GraphicsCaptureApiHandler>::start_free_threaded(crate_settings);
}

fn is_unsupported_settings(e: &GraphicsCaptureApiError<BoxError>) -> bool {
    return matches!(
        e,
        GraphicsCaptureApiError::GraphicsCaptureApiError(
            CaptureApiError::CursorConfigUnsupported
                | CaptureApiError::BorderConfigUnsupported
                | CaptureApiError::SecondaryWindowsUnsupported
                | CaptureApiError::MinimumUpdateIntervalUnsupported
        )
    );
}

/// Active Windows Graphics Capture session.
pub struct WindowsCaptureSource {
    source_ref: SourceRef,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    control: Mutex<Option<CaptureControl<CaptureHandler, BoxError>>>,
}

impl WindowsCaptureSource {
    pub fn spawn(source_ref: SourceRef, cfg: &WindowsCaptureSourceConfig) -> Self {
        let slot = Arc::new(Mutex::new(None::<Frame>));
        let stats = Arc::new(Mutex::new(SourceStats::new()));

        let control = match resolve_target(cfg) {
            Ok(CaptureItem::Monitor(monitor)) => start_capture(
                &source_ref,
                monitor,
                cfg.settings(),
                false,
                slot.clone(),
                stats.clone(),
            ),
            Ok(CaptureItem::Window(window)) => start_capture(
                &source_ref,
                window,
                cfg.settings(),
                true,
                slot.clone(),
                stats.clone(),
            ),
            Err(e) => {
                tracing::error!(source = %source_ref, "Windows Capture target not found: {e}");
                None
            }
        };

        return Self {
            source_ref,
            slot,
            stats,
            control: Mutex::new(control),
        };
    }
}

impl Drop for WindowsCaptureSource {
    fn drop(&mut self) {
        let control = self.control.get_mut().ok().and_then(|slot| slot.take());
        if let Some(control) = control
            && let Err(e) = control.stop()
        {
            tracing::error!(
                source = %self.source_ref,
                "Failed to stop Windows capture: {e}"
            );
        }
    }
}

impl VideoSource for WindowsCaptureSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        return self.slot.lock().unwrap().clone();
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }
}
