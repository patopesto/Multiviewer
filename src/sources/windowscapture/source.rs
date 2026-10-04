use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use windows_capture::capture::{
    CaptureControl, Context, GraphicsCaptureApiError, GraphicsCaptureApiHandler,
};
use windows_capture::frame::Frame as WgcFrame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use super::super::{CpuFrame, Frame, FramePool, PixelFormat, SourceRef, SourceStats, VideoSource};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Capture target, tagged on the wire as `kind`; older flat configs do not parse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WindowsCaptureSourceConfig {
    Display {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        device_name: String,
    },
    Window {
        hwnd: u64,
        process_name: String,
        title: String,
    },
}

impl Default for WindowsCaptureSourceConfig {
    /// Placeholder for a source whose target has not been picked yet.
    fn default() -> Self {
        return Self::Display {
            device_name: String::new(),
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
        WindowsCaptureSourceConfig::Display { device_name } => {
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

fn start_capture<T>(
    item: T,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
) -> Result<CaptureControl<CaptureHandler, BoxError>, GraphicsCaptureApiError<BoxError>>
where
    T: TryInto<GraphicsCaptureItemType> + Send + 'static,
{
    let settings = Settings::new(
        item,
        CursorCaptureSettings::Default,
        DrawBorderSettings::Default,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        (slot, stats),
    );

    return <CaptureHandler as GraphicsCaptureApiHandler>::start_free_threaded(settings);
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
        let control = Mutex::new(None);

        match resolve_target(cfg) {
            Ok(CaptureItem::Monitor(monitor)) => {
                match start_capture(monitor, slot.clone(), stats.clone()) {
                    Ok(c) => *control.lock().unwrap() = Some(c),
                    Err(e) => {
                        tracing::error!(
                            source = %source_ref,
                            "Failed to start Windows monitor capture: {e}"
                        );
                    }
                }
            }
            Ok(CaptureItem::Window(window)) => {
                match start_capture(window, slot.clone(), stats.clone()) {
                    Ok(c) => *control.lock().unwrap() = Some(c),
                    Err(e) => {
                        tracing::error!(
                            source = %source_ref,
                            "Failed to start Windows window capture: {e}"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::error!(source = %source_ref, "Windows Capture target not found: {e}");
            }
        }

        return Self {
            source_ref,
            slot,
            stats,
            control,
        };
    }
}

impl Drop for WindowsCaptureSource {
    fn drop(&mut self) {
        let control = self.control.get_mut().ok().and_then(|slot| slot.take());
        if let Some(control) = control {
            if let Err(e) = control.stop() {
                tracing::error!(
                    source = %self.source_ref,
                    "Failed to stop Windows capture: {e}"
                );
            }
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
