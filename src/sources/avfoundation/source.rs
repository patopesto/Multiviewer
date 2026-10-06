use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use av_foundation::{
    capture_device::{AVCaptureDevice, AVCaptureDeviceFormat},
    capture_input::AVCaptureDeviceInput,
    capture_output_base::AVCaptureOutput,
    capture_session::{AVCaptureConnection, AVCaptureSession},
    capture_video_data_output::{
        AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate,
    },
};
use core_foundation::base::TCFType;
use core_media::format_description::CMVideoFormatDescription;
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_media::time::CMTime;
use core_video::pixel_buffer::{
    kCVPixelBufferLock_ReadOnly, kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA,
    CVPixelBuffer,
};
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_foundation::{NSObject, NSObjectProtocol, NSNumber, NSString};

use super::super::{CpuFrame, Frame, FramePool, PixelFormat, SourceRef, SourceStats, VideoSource};

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MVAvFoundationCaptureDelegate"]
    #[ivars = (RefCell<Arc<Mutex<Option<Frame>>>>, RefCell<Arc<Mutex<SourceStats>>>, Cell<u64>, RefCell<FramePool>)]
    struct CaptureDelegate;

    impl CaptureDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars((
                RefCell::new(Arc::new(Mutex::new(None))),
                RefCell::new(Arc::new(Mutex::new(SourceStats::new()))),
                Cell::new(0),
                RefCell::new(FramePool::new()),
            ));
            unsafe { msg_send![super(this), init] }
        }
    }

    unsafe impl NSObjectProtocol for CaptureDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CaptureDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        #[instrument(name = "avfoundation_frame", level = "debug", skip_all)]
        unsafe fn capture_output_did_output_sample_buffer(
            &self,
            _capture_output: &AVCaptureOutput,
            sample_buffer: CMSampleBufferRef,
            _connection: &AVCaptureConnection,
        ) {
            let sample_buffer =
                unsafe { CMSampleBuffer::wrap_under_get_rule(sample_buffer) };
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
            // One bulk copy at the buffer's native row stride; the compositor
            // passes the stride straight to write_texture instead of us
            // repacking every row each frame.
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
);

impl CaptureDelegate {
    fn new(slot: Arc<Mutex<Option<Frame>>>, stats: Arc<Mutex<SourceStats>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars((
            RefCell::new(slot),
            RefCell::new(stats),
            Cell::new(0),
            RefCell::new(FramePool::new()),
        ));
        let this: Option<Retained<Self>> = unsafe { msg_send![super(this), init] };
        this.expect("AVFoundation capture delegate init failed")
    }
}


/// One mode the device advertises: a format's dimensions at its fastest rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CaptureMode {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

impl CaptureMode {
    /// Human-readable `WxH @ fps` label; decimals only for non-integral rates.
    pub fn label(&self) -> String {
        let fps = if self.fps_num == 0 || self.fps_den == 0 {
            "?".to_string()
        } else {
            let value = self.fps_num as f64 / self.fps_den as f64;
            // A non-reduced ratio (e.g. 30000000/1000000 = 30) still prints whole.
            if (value - value.round()).abs() < 1e-6 {
                format!("{}", value.round() as u64)
            } else {
                format!("{value:.2}")
            }
        };
        return format!("{}x{} @ {}", self.width, self.height, fps);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AvFoundationSourceConfig {
    pub device_unique_id: String,
    /// Requested frame width in pixels; 0 lets the device pick its default.
    #[serde(default)]
    pub width: u32,
    /// Requested frame height in pixels; 0 lets the device pick its default.
    #[serde(default)]
    pub height: u32,
    /// Requested frame-rate numerator; 0 lets the device pick its default.
    #[serde(default)]
    pub fps_num: u32,
    /// Requested frame-rate denominator (stored exactly, e.g. 1001 for 29.97).
    #[serde(default)]
    pub fps_den: u32,
    /// Whether the output discards frames delivered late.
    #[serde(default = "drop_late_frames_default")]
    pub drop_late_frames: bool,
}

/// Discard late frames by default, matching the low-latency frame handoff.
fn drop_late_frames_default() -> bool {
    return true;
}

impl Default for AvFoundationSourceConfig {
    fn default() -> Self {
        Self {
            device_unique_id: String::new(),
            width: 0,
            height: 0,
            fps_num: 0,
            fps_den: 0,
            drop_late_frames: true,
        }
    }
}

/// Active AVFoundation capture source on its own thread.
pub struct AvFoundationSource {
    source_ref: SourceRef,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AvFoundationSource {
    pub fn spawn(
        source_ref: SourceRef,
        cfg: &AvFoundationSourceConfig,
        modes: Arc<Mutex<Vec<CaptureMode>>>,
    ) -> Self {
        let slot = Arc::new(Mutex::new(None::<Frame>));
        let stats = Arc::new(Mutex::new(SourceStats::new()));

        let slot2 = slot.clone();
        let stats2 = stats.clone();
        let trace_ref = source_ref.clone();
        let config = cfg.clone();
        let running = Arc::new(AtomicBool::new(true));
        let running2 = running.clone();

        let thread = std::thread::Builder::new()
            .name(format!("avf-in-{source_ref}"))
            .spawn(move || {
                run_capture(trace_ref, config, slot2, stats2, running2, modes);
            })
            .expect("spawn avfoundation capture thread");

        Self {
            source_ref,
            slot,
            stats,
            running,
            thread: Some(thread),
        }
    }
}

/// Run the capture session on the source's thread until `running` clears.
fn run_capture(
    source_ref: String,
    cfg: AvFoundationSourceConfig,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    modes: Arc<Mutex<Vec<CaptureMode>>>,
) {
    let unique_id = NSString::from_str(&cfg.device_unique_id);
    let Some(device) = AVCaptureDevice::device_with_unique_id(&unique_id) else {
        tracing::error!(source=source_ref, "AVFoundation device not found: {}", cfg.device_unique_id);
        return;
    };

    let session = AVCaptureSession::new();
    *modes.lock().unwrap() = enumerate_modes(&device);

    let input = match AVCaptureDeviceInput::from_device(&device) {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(source=source_ref, "AVFoundation could not create device input for {}: {}", cfg.device_unique_id, e);
            return;
        }
    };

    let output = AVCaptureVideoDataOutput::new();
    output.set_always_discards_late_video_frames(cfg.drop_late_frames);

    let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let key: &NSString = unsafe { &*(kCVPixelBufferPixelFormatTypeKey as *const NSString) };
    let settings = objc2_foundation::NSDictionary::from_slices(
        &[key],
        &[format.as_ref() as &objc2_foundation::NSObject],
    );
    output.set_video_settings(&settings);

    let delegate = CaptureDelegate::new(slot, stats);
    let delegate_obj: &ProtocolObject<dyn AVCaptureVideoDataOutputSampleBufferDelegate> =
        ProtocolObject::from_ref(&*delegate);
    let queue = DispatchQueue::new("net.bambinito.multiviewer.avfoundation", DispatchQueueAttr::SERIAL);
    output.set_sample_buffer_delegate(delegate_obj, &queue);

    // On macOS the session's preset re-picks the device format at
    // startRunning, so the device must stay locked across it.
    let locked = lock_and_apply_mode(&device, &cfg, &source_ref);

    session.begin_configuration();
    session.add_input(&input);
    session.add_output(&output);
    session.commit_configuration();

    let _delegate = delegate;
    session.start_running();

    if locked {
        device.unlock_for_configuration();
    }
    log_active_format(&device, &source_ref);

    while running.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }

    session.stop_running();
}

/// The device's advertised modes: each format's dimensions at its fastest
/// supported rate, deduped and sorted.
fn enumerate_modes(device: &AVCaptureDevice) -> Vec<CaptureMode> {
    let mut modes = Vec::new();
    for format in device.formats().iter() {
        let Some(desc) = format.format_description().downcast::<CMVideoFormatDescription>() else {
            continue;
        };
        let dims = desc.get_dimensions();
        if dims.width <= 0 || dims.height <= 0 {
            continue;
        }
        for range in format.video_supported_frame_rate_ranges().iter() {
            // The shortest frame duration is the fastest rate; its exact
            // rational keeps NTSC rates (e.g. 30000/1001) intact.
            let duration = range.min_frame_duration();
            if duration.value <= 0 || duration.timescale <= 0 {
                continue;
            }
            modes.push(CaptureMode {
                width: dims.width as u32,
                height: dims.height as u32,
                fps_num: duration.timescale as u32,
                fps_den: duration.value as u32,
            });
        }
    }
    modes.sort_unstable();
    modes.dedup();
    return modes;
}

/// The device format with the requested dimensions, if the device advertises one.
fn find_format(device: &AVCaptureDevice, width: u32, height: u32) -> Option<Retained<AVCaptureDeviceFormat>> {
    return device.formats().iter().find(|format| {
        let Some(desc) = format.format_description().downcast::<CMVideoFormatDescription>() else {
            return false;
        };
        let dims = desc.get_dimensions();
        dims.width as u32 == width && dims.height as u32 == height
    });
}

/// Lock the device and apply the requested format and frame rate, leaving the
/// lock held so the session cannot override the format at `startRunning`.
/// Returns whether the device is now locked; the caller must unlock it.
fn lock_and_apply_mode(device: &AVCaptureDevice, cfg: &AvFoundationSourceConfig, source_ref: &str) -> bool {
    if cfg.width == 0 || cfg.height == 0 {
        return false;
    }
    let Some(format) = find_format(device, cfg.width, cfg.height) else {
        tracing::warn!(source = source_ref, "AVFoundation device has no {}x{} format", cfg.width, cfg.height);
        return false;
    };
    if device.lock_for_configuration().is_err() {
        tracing::error!(source = source_ref, "AVFoundation could not lock device for configuration");
        return false;
    }
    device.set_active_format(&format);
    if cfg.fps_num > 0 && cfg.fps_den > 0 {
        if format_supports_rate(&format, cfg.fps_num, cfg.fps_den) {
            // Lock both bounds so the device cannot pick a different rate.
            let duration = CMTime::make(cfg.fps_den as i64, cfg.fps_num as i32);
            device.set_active_video_min_frame_duration(duration);
            device.set_active_video_max_frame_duration(duration);
        } else {
            // An unsupported duration throws an ObjC exception, so leave the
            // device at its default rate for this format.
            tracing::warn!(source = source_ref, "AVFoundation {}x{} has no {} fps rate", cfg.width, cfg.height, cfg.fps_num as f64 / cfg.fps_den as f64);
        }
    }
    return true;
}

/// Log the device's negotiated format, so a mode that did not stick is visible.
fn log_active_format(device: &AVCaptureDevice, source_ref: &str) {
    let desc = device.get_active_format().format_description();
    let Some(desc) = desc.downcast::<CMVideoFormatDescription>() else {
        return;
    };
    let dims = desc.get_dimensions();
    let duration = device.get_active_video_min_frame_duration();
    let fps = if duration.value > 0 && duration.timescale > 0 {
        duration.timescale as f64 / duration.value as f64
    } else {
        0.0
    };
    tracing::info!(source = source_ref, "AVFoundation active format: {}x{} ~{:.2} fps", dims.width, dims.height, fps);
}

/// Whether `format` advertises a frame-rate range containing `fps_num / fps_den`.
fn format_supports_rate(format: &AVCaptureDeviceFormat, fps_num: u32, fps_den: u32) -> bool {
    let fps = fps_num as f64 / fps_den as f64;
    return format.video_supported_frame_rate_ranges().iter().any(|range| {
        fps >= range.min_frame_rate() - 0.01 && fps <= range.max_frame_rate() + 0.01
    });
}

impl Drop for AvFoundationSource {
    fn drop(&mut self) {
        self.running
            .store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() && let Err(e) = t.join() {
            tracing::error!(source=self.source_ref, "Thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for AvFoundationSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        return self.slot.lock().unwrap().clone();
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config saved before the tunables existed keeps low-latency defaults.
    #[test]
    fn legacy_config_defaults_new_tunables() {
        let cfg: AvFoundationSourceConfig =
            serde_json::from_str(r#"{"device_unique_id":"0x1"}"#).unwrap();
        assert_eq!(cfg.width, 0);
        assert_eq!(cfg.height, 0);
        assert_eq!(cfg.fps_num, 0);
        assert_eq!(cfg.fps_den, 0);
        assert!(cfg.drop_late_frames);
    }

    #[test]
    fn config_round_trips_mode_and_late_frames() {
        let cfg = AvFoundationSourceConfig {
            device_unique_id: "0x1".to_string(),
            width: 1920,
            height: 1080,
            fps_num: 30000,
            fps_den: 1001,
            drop_late_frames: false,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert_eq!(serde_json::from_str::<AvFoundationSourceConfig>(&json).unwrap(), cfg);
    }

    #[test]
    fn mode_label_renders_integral_and_fractional_rates() {
        let integral = CaptureMode { width: 1280, height: 720, fps_num: 30, fps_den: 1 };
        assert_eq!(integral.label(), "1280x720 @ 30");
        let ntsc = CaptureMode { width: 1920, height: 1080, fps_num: 30000, fps_den: 1001 };
        assert_eq!(ntsc.label(), "1920x1080 @ 29.97");
        // An unreduced 30 fps ratio must still print whole.
        let unreduced = CaptureMode { width: 1280, height: 720, fps_num: 30_000_000, fps_den: 1_000_000 };
        assert_eq!(unreduced.label(), "1280x720 @ 30");
    }
}
