use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_graphics2::display::CGDisplay;
use core_graphics2::geometry::CGRect;
use core_graphics2::window::{
    copy_window_info, preflight_screen_capture_access, CGWindowListOption, kCGWindowNumber,
};
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_video::pixel_buffer::{
    kCVPixelBufferLock_ReadOnly, kCVPixelFormatType_32BGRA, CVPixelBuffer,
};
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use screen_capture_kit::shareable_content::SCShareableContent;
use screen_capture_kit::stream::{
    SCContentFilter, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput,
    SCStreamOutputType,
};

use super::super::{CpuFrame, Frame, FramePool, PixelFormat, SourceRef, SourceStats, VideoSource};
use super::discovery::fetch_content;

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MVScreenCaptureKitStreamDelegate"]
    #[ivars = (RefCell<Arc<Mutex<Option<Frame>>>>, RefCell<Arc<Mutex<SourceStats>>>, Cell<u64>, RefCell<FramePool>)]
    struct StreamDelegate;

    unsafe impl NSObjectProtocol for StreamDelegate {}

    unsafe impl SCStreamOutput for StreamDelegate {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        #[instrument(name = "screencapturekit_frame", level = "debug", skip_all)]
        unsafe fn stream_did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: CMSampleBufferRef,
            of_type: SCStreamOutputType,
        ) {
            if of_type != SCStreamOutputType::Screen {
                return;
            }

            let sample_buffer = unsafe { CMSampleBuffer::wrap_under_get_rule(sample_buffer) };
            let Some(image_buffer) = sample_buffer.get_image_buffer() else {
                return;
            };
            let Some(pixel_buffer) = image_buffer.downcast::<CVPixelBuffer>() else {
                return;
            };

            let w = pixel_buffer.get_width() as u32;
            let h = pixel_buffer.get_height() as u32;
            if pixel_buffer.get_pixel_format() != kCVPixelFormatType_32BGRA {
                return;
            }

            let bytes_per_row = pixel_buffer.get_bytes_per_row();
            let rows = h as usize;
            // One bulk copy at the native stride; the compositor uploads it as-is.
            let copy_len = (bytes_per_row * rows).min(pixel_buffer.get_data_size());
            let t0 = Instant::now();

            let data = unsafe {
                if pixel_buffer.lock_base_address(kCVPixelBufferLock_ReadOnly) != 0 {
                    return;
                }
                let src = pixel_buffer.get_base_address() as *const u8;
                let mut buf = self.ivars().3.borrow_mut().take(copy_len);
                std::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), copy_len);
                let _ = pixel_buffer.unlock_base_address(kCVPixelBufferLock_ReadOnly);
                buf
            };

            let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;
            let ivars = self.ivars();
            let seq = ivars.2.get();
            ivars.2.set(seq + 1);

            {
                let stats_arc = ivars.1.borrow();
                let mut s = stats_arc.lock().unwrap();
                s.record_frame(w, h, PixelFormat::Bgra8.label(), 0.0);
                s.record_copy_time(copy_ms);
            }

            let frame = Frame::Cpu(CpuFrame {
                data: Arc::new(data),
                w,
                h,
                fmt: PixelFormat::Bgra8,
                pitch: bytes_per_row as u32,
                seq,
            });
            let slot_arc = ivars.0.borrow();
            let old = slot_arc.lock().unwrap().replace(frame);
            drop(slot_arc);
            ivars.3.borrow_mut().give(old);
        }
    }

    unsafe impl SCStreamDelegate for StreamDelegate {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn stream_did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            tracing::error!(
                "ScreenCaptureKit stream stopped: {}",
                error.localizedDescription()
            );
        }
    }
);

impl StreamDelegate {
    fn new(slot: Arc<Mutex<Option<Frame>>>, stats: Arc<Mutex<SourceStats>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars((
            RefCell::new(slot),
            RefCell::new(stats),
            Cell::new(0),
            RefCell::new(FramePool::new()),
        ));
        let this: Option<Retained<Self>> = unsafe { msg_send![super(this), init] };
        this.expect("ScreenCaptureKit stream delegate init failed")
    }
}

/// Capture target, tagged on the wire as `kind`; older flat configs do not parse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScreenCaptureKitSourceConfig {
    Display {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        display_id: String,
    },
    Window {
        window_id: u32,
        bundle_id: String,
        title: String,
    },
}

impl Default for ScreenCaptureKitSourceConfig {
    /// Placeholder for a source whose target has not been picked yet.
    fn default() -> Self {
        return Self::Display {
            display_id: String::new(),
        };
    }
}

/// Identity of a running stream; a changed key means stop + start.
#[derive(Debug, PartialEq, Eq)]
enum StreamKey {
    Display { display_id: String, w: usize, h: usize },
    Window { window_id: u32, w: usize, h: usize },
}

/// A running stream, dropped after `stop_stream`.
struct ActiveStream {
    key: StreamKey,
    stream: Retained<SCStream>,
    _delegate: Retained<StreamDelegate>,
}

/// Active ScreenCaptureKit stream (display or window) on its own thread.
pub struct ScreenCaptureKitSource {
    source_ref: SourceRef,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ScreenCaptureKitSource {
    pub fn spawn(source_ref: SourceRef, cfg: &ScreenCaptureKitSourceConfig) -> Self {
        let slot = Arc::new(Mutex::new(None::<Frame>));
        let stats = Arc::new(Mutex::new(SourceStats::new()));

        let slot2 = slot.clone();
        let stats2 = stats.clone();
        let trace_ref = source_ref.clone();
        let config = cfg.clone();
        let running = Arc::new(AtomicBool::new(true));
        let running2 = running.clone();

        let thread = std::thread::Builder::new()
            .name(format!("screencapturekit-in-{source_ref}"))
            .spawn(move || {
                Self::run_capture(trace_ref, config, slot2, stats2, running2);
            })
            .expect("spawn screencapturekit capture thread");

        Self {
            source_ref,
            slot,
            stats,
            running,
            thread: Some(thread),
        }
    }

    fn run_capture(
        source_ref: String,
        config: ScreenCaptureKitSourceConfig,
        slot: Arc<Mutex<Option<Frame>>>,
        stats: Arc<Mutex<SourceStats>>,
        running: Arc<AtomicBool>,
    ) {
        let queue = DispatchQueue::new(
            "net.bambinito.multiviewer.screencapturekit",
            DispatchQueueAttr::SERIAL,
        );
        let mut active: Option<ActiveStream> = None;
        // The poll repeats every second, so each distinct error logs once.
        let mut last_error: Option<String> = None;
        // Re-resolve each second: restart on size change, freeze when off-screen.
        while running.load(Ordering::Relaxed) {
            // Preflight never prompts; SCShareableContent does — stay out of SCK while denied.
            let resolved = if !preflight_screen_capture_access() {
                Err("screen recording permission not granted".to_string())
            } else {
                match fetch_content() {
                    Ok(content) => resolve_target(&content, &config),
                    Err(e) => Err(e),
                }
            };
            match resolved {
                Ok((key, filter, out_w, out_h)) => {
                    last_error = None;
                    stats.lock().unwrap().set_off_screen(false);
                    if active.as_ref().is_none_or(|a| a.key != key) {
                        if let Some(a) = active.take() {
                            stop_stream(&a.stream, &source_ref);
                        }
                        match start_stream(
                            &source_ref,
                            key,
                            filter,
                            out_w,
                            out_h,
                            &slot,
                            &stats,
                            &queue,
                        ) {
                            Ok(stream) => active = Some(stream),
                            Err(e) => {
                                note_error(&mut last_error, e, &source_ref);
                                stats.lock().unwrap().set_off_screen(true);
                            }
                        }
                    }
                }
                // Target missing or off-screen: stop and keep the last frame.
                Err(e) => {
                    note_error(&mut last_error, e, &source_ref);
                    if let Some(a) = active.take() {
                        stop_stream(&a.stream, &source_ref);
                    }
                    stats.lock().unwrap().set_off_screen(true);
                }
            }
            if !sleep_while_running(&running, Duration::from_secs(1)) {
                break;
            }
        }

        if let Some(a) = active {
            stop_stream(&a.stream, &source_ref);
        }
    }
}

impl Drop for ScreenCaptureKitSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() && let Err(e) = t.join() {
            tracing::error!(source=self.source_ref, "Thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for ScreenCaptureKitSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        self.slot.lock().unwrap().clone()
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        self.stats.clone()
    }
}

/// Filter + output size + key for the current content; `Err` = un-capturable, hold the last frame.
fn resolve_target(
    content: &Retained<SCShareableContent>,
    config: &ScreenCaptureKitSourceConfig,
) -> Result<(StreamKey, Retained<SCContentFilter>, usize, usize), String> {
    let (key, filter, out_w, out_h) = match config {
        ScreenCaptureKitSourceConfig::Window {
            window_id,
            bundle_id,
            title,
        } => {
            let windows = content.windows();
            let window = windows
                .iter()
                .find(|w| w.window_id() == *window_id)
                .or_else(|| {
                    // Window id went stale (app relaunched): re-find by app + title.
                    windows.iter().find(|w| {
                        w.owning_application()
                            .is_some_and(|a| a.bundle_identifier().to_string() == *bundle_id)
                            && w.title().is_some_and(|t| t.to_string() == *title)
                    })
                });
            let Some(window) = window else {
                return Err(format!("window {window_id} not found"));
            };
            // Off-Space or minimized window renders blank: hold the last frame.
            if !window_on_screen(window.window_id()) {
                return Err(format!("window {window_id} is not on a visible desktop"));
            }
            let (w, h) = window_output_size(content, &window.frame());
            let filter = SCContentFilter::init_with_desktop_independent_window(
                SCContentFilter::alloc(),
                &window,
            );
            let key = StreamKey::Window {
                window_id: window.window_id(),
                w,
                h,
            };
            (key, filter, w, h)
        }
        ScreenCaptureKitSourceConfig::Display { display_id } => {
            let displays = content.displays();
            let Some(display) = displays
                .iter()
                .find(|d| d.display_id().to_string() == *display_id)
            else {
                return Err(format!("display {display_id} not found"));
            };
            let filter = SCContentFilter::init_with_display_exclude_windows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            );
            let (w, h) = (display.width() as usize, display.height() as usize);
            let key = StreamKey::Display {
                display_id: display_id.clone(),
                w,
                h,
            };
            (key, filter, w, h)
        }
    };
    return Ok((key, filter, out_w, out_h));
}

/// Frame points × the display's pixel scale, so HiDPI windows stream sharp.
fn window_output_size(content: &Retained<SCShareableContent>, frame: &CGRect) -> (usize, usize) {
    let (x, y, w, h) = (frame.origin.x, frame.origin.y, frame.size.width, frame.size.height);
    let mut best_overlap = 0.0;
    let mut scale = 1.0;
    for display in content.displays().iter() {
        let d = display.frame();
        let iw = (d.origin.x + d.size.width).min(x + w) - d.origin.x.max(x);
        let ih = (d.origin.y + d.size.height).min(y + h) - d.origin.y.max(y);
        if iw <= 0.0 || ih <= 0.0 {
            continue;
        }
        if iw * ih > best_overlap {
            best_overlap = iw * ih;
            scale = display_scale(display.display_id());
        }
    }
    let ow = (w * scale).round().max(1.0) as usize;
    let oh = (h * scale).round().max(1.0) as usize;
    return (ow, oh);
}

/// Points→pixels ratio of one display (1.0 when it cannot be determined).
fn display_scale(display_id: u32) -> f64 {
    let display = CGDisplay::new(display_id);
    let points = display.bounds().size.width;
    if points <= 0.0 {
        return 1.0;
    }
    let scale = display.pixels_wide() as f64 / points;
    return if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
}

/// Whether the window is on a visible desktop — SCK feeds blank frames otherwise.
fn window_on_screen(window_id: u32) -> bool {
    let Some(list) = copy_window_info(CGWindowListOption::OnScreenOnly, 0) else {
        return false;
    };
    // Untyped CFDictionary: only form with the downcast bound; key matched by CFEqual.
    let key: *const c_void = unsafe { kCGWindowNumber as *const c_void };
    for item in list.iter() {
        let Some(dict) = unsafe { CFType::wrap_under_get_rule(*item) }.downcast::<CFDictionary>()
        else {
            continue;
        };
        let Some(value) = dict.find(key) else {
            continue;
        };
        if let Some(num) = unsafe { CFType::wrap_under_get_rule(*value) }.downcast::<CFNumber>()
            && num.to_i64().map(|i| i as u32) == Some(window_id)
        {
            return true;
        }
    }
    return false;
}

/// Build and start a stream; errors go back to the poll loop to dedup + retry.
#[allow(clippy::too_many_arguments)]
fn start_stream(
    source_ref: &str,
    key: StreamKey,
    filter: Retained<SCContentFilter>,
    out_w: usize,
    out_h: usize,
    slot: &Arc<Mutex<Option<Frame>>>,
    stats: &Arc<Mutex<SourceStats>>,
    queue: &DispatchQueue,
) -> Result<ActiveStream, String> {
    let configuration = SCStreamConfiguration::new();
    configuration.set_width(out_w);
    configuration.set_height(out_h);
    configuration.set_pixel_format(kCVPixelFormatType_32BGRA);
    // Output size is already in pixels; scaling keeps a mismatch from cropping.
    configuration.set_scales_to_fit(true);

    let delegate = StreamDelegate::new(slot.clone(), stats.clone());
    let output: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*delegate);
    let stream_delegate: &ProtocolObject<dyn SCStreamDelegate> =
        ProtocolObject::from_ref(&*delegate);
    let stream = SCStream::init_with_filter(
        SCStream::alloc(),
        &filter,
        &configuration,
        stream_delegate,
    );

    if let Err(e) = stream.add_stream_output(output, SCStreamOutputType::Screen, queue) {
        return Err(format!(
            "could not add stream output: {}",
            e.localizedDescription()
        ));
    }

    let start_ref = source_ref.to_string();
    stream.start_capture(move |error| {
        if let Some(error) = error {
            tracing::error!(
                source = start_ref,
                "ScreenCaptureKit start capture failed: {}",
                error.localizedDescription()
            );
        }
    });

    return Ok(ActiveStream {
        key,
        stream,
        _delegate: delegate,
    });
}

fn stop_stream(stream: &SCStream, source_ref: &str) {
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    stream.stop_capture(move |error| {
        let _ = stop_tx.send(error);
    });
    match stop_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Some(error)) => tracing::error!(
            source = source_ref,
            "ScreenCaptureKit stop capture failed: {}",
            error.localizedDescription()
        ),
        Ok(None) => {}
        Err(_) => {
            tracing::warn!(source = source_ref, "ScreenCaptureKit stop capture timed out");
        }
    }
}

/// Log `e` only when it differs from the last one — the poll repeats it.
fn note_error(last: &mut Option<String>, e: String, source_ref: &str) {
    if last.as_deref() != Some(e.as_str()) {
        tracing::error!(source = source_ref, "ScreenCaptureKit: {e}");
        *last = Some(e);
    }
}

/// Sleep in 100 ms slices; false once stopped, so `Drop` joins promptly.
fn sleep_while_running(running: &Arc<AtomicBool>, total: Duration) -> bool {
    let mut left = total;
    while left > Duration::ZERO {
        if !running.load(Ordering::Relaxed) {
            return false;
        }
        let step = left.min(Duration::from_millis(100));
        std::thread::sleep(step);
        left -= step;
    }
    return running.load(Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_key_detects_target_change() {
        let a = StreamKey::Window { window_id: 1, w: 800, h: 600 };
        // Same window and size: no restart.
        assert_eq!(a, StreamKey::Window { window_id: 1, w: 800, h: 600 });
        // Resized: restart.
        assert_ne!(a, StreamKey::Window { window_id: 1, w: 1024, h: 768 });
        // Moved to another display with a different scale: restart.
        assert_ne!(a, StreamKey::Window { window_id: 1, w: 1600, h: 1200 });
        // A different window: restart.
        assert_ne!(a, StreamKey::Window { window_id: 2, w: 800, h: 600 });
        // Display vs window: restart.
        assert_ne!(
            a,
            StreamKey::Display { display_id: "1".into(), w: 800, h: 600 }
        );
    }

    /// Drop must interrupt the poll, not wait out the one-second cadence.
    #[test]
    fn poll_sleep_returns_false_once_stopped() {
        let running = Arc::new(AtomicBool::new(false));
        let t0 = Instant::now();
        assert!(!sleep_while_running(&running, Duration::from_secs(30)));
        assert!(t0.elapsed() < Duration::from_secs(1));
    }
}
