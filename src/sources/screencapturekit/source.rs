use super::super::{CpuFrame, Frame, PixelFormat, SourceRef, SourceStats, VideoSource};
use super::discovery::fetch_content;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

use core_foundation::base::TCFType;
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_video::pixel_buffer::{
    kCVPixelBufferLock_ReadOnly, kCVPixelFormatType_32BGRA, CVPixelBuffer,
};
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use screen_capture_kit::stream::{
    SCContentFilter, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput,
    SCStreamOutputType,
};

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MVScreenCaptureKitStreamDelegate"]
    #[ivars = (RefCell<Arc<Mutex<Option<Frame>>>>, RefCell<Arc<Mutex<SourceStats>>>, Cell<u64>)]
    struct StreamDelegate;

    unsafe impl NSObjectProtocol for StreamDelegate {}

    unsafe impl SCStreamOutput for StreamDelegate {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
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
            let expected = (w as usize) * (h as usize) * 4;
            let t0 = Instant::now();

            let data = unsafe {
                if pixel_buffer.lock_base_address(kCVPixelBufferLock_ReadOnly) != 0 {
                    return;
                }
                let src = pixel_buffer.get_base_address() as *const u8;
                let mut packed = vec![0u8; expected];
                if bytes_per_row == (w as usize) * 4 {
                    std::ptr::copy_nonoverlapping(src, packed.as_mut_ptr(), expected);
                } else {
                    for y in 0..(h as usize) {
                        let src_row = src.add(y * bytes_per_row);
                        let dst_row = packed.as_mut_ptr().add(y * (w as usize) * 4);
                        std::ptr::copy_nonoverlapping(src_row, dst_row, (w as usize) * 4);
                    }
                }
                let _ = pixel_buffer.unlock_base_address(kCVPixelBufferLock_ReadOnly);
                packed
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
                seq,
            });
            let slot_arc = ivars.0.borrow();
            *slot_arc.lock().unwrap() = Some(frame);
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
        ));
        let this: Option<Retained<Self>> = unsafe { msg_send![super(this), init] };
        this.expect("ScreenCaptureKit stream delegate init failed")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScreenCaptureKitSourceConfig {
    pub display_id: String,
}

/// Active ScreenCaptureKit display stream on its own thread.
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
        let display_id = cfg.display_id.clone();
        let running = Arc::new(AtomicBool::new(true));
        let running2 = running.clone();

        let thread = std::thread::Builder::new()
            .name(format!("screencapturekit-in-{source_ref}"))
            .spawn(move || {
                Self::run_capture(trace_ref, display_id, slot2, stats2, running2);
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
        display_id: String,
        slot: Arc<Mutex<Option<Frame>>>,
        stats: Arc<Mutex<SourceStats>>,
        running: Arc<AtomicBool>,
    ) {
        let content = match fetch_content() {
            Ok(content) => content,
            Err(e) => {
                tracing::error!(source=source_ref, "ScreenCaptureKit discovery failed: {}", e);
                return;
            }
        };
        let displays = content.displays();
        let Some(display) = displays
            .iter()
            .find(|d| d.display_id().to_string() == display_id)
        else {
            tracing::error!(source=source_ref, "ScreenCaptureKit display not found: {}", display_id);
            return;
        };

        let filter = SCContentFilter::init_with_display_exclude_windows(
            SCContentFilter::alloc(),
            &display,
            &NSArray::new(),
        );

        let configuration = SCStreamConfiguration::new();
        configuration.set_width(display.width() as usize);
        configuration.set_height(display.height() as usize);
        configuration.set_pixel_format(kCVPixelFormatType_32BGRA);

        let delegate = StreamDelegate::new(slot, stats);
        let output: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*delegate);
        let stream_delegate: &ProtocolObject<dyn SCStreamDelegate> =
            ProtocolObject::from_ref(&*delegate);
        let stream = SCStream::init_with_filter(
            SCStream::alloc(),
            &filter,
            &configuration,
            stream_delegate,
        );

        let queue = DispatchQueue::new(
            "net.bambinito.multiviewer.screencapturekit",
            DispatchQueueAttr::SERIAL,
        );
        if let Err(e) = stream.add_stream_output(output, SCStreamOutputType::Screen, &queue) {
            tracing::error!(
                source=source_ref,
                "ScreenCaptureKit could not add stream output: {}",
                e.localizedDescription()
            );
            return;
        }

        let start_ref = source_ref.clone();
        stream.start_capture(move |error| {
            if let Some(error) = error {
                tracing::error!(
                    source=start_ref,
                    "ScreenCaptureKit start capture failed: {}",
                    error.localizedDescription()
                );
            }
        });

        // Keep the delegate and stream alive for as long as the stream runs.
        let _delegate = delegate;
        let _stream = stream;

        while running.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }

        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        _stream.stop_capture(move |error| {
            let _ = stop_tx.send(error);
        });
        match stop_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Some(error)) => tracing::error!(
                source=source_ref,
                "ScreenCaptureKit stop capture failed: {}",
                error.localizedDescription()
            ),
            Ok(None) => {}
            Err(_) => {
                tracing::warn!(source=source_ref, "ScreenCaptureKit stop capture timed out");
            }
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
