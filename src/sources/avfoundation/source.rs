use super::super::{CpuFrame, Frame, PixelFormat, SourceStats, VideoSource};
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

use av_foundation::{
    capture_device::AVCaptureDevice,
    capture_input::AVCaptureDeviceInput,
    capture_output_base::AVCaptureOutput,
    capture_session::{AVCaptureConnection, AVCaptureSession},
    capture_video_data_output::{
        AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate,
    },
};
use core_foundation::base::TCFType;
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_video::pixel_buffer::{
    kCVPixelBufferLock_ReadOnly, kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA,
    CVPixelBuffer,
};
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_foundation::{NSObject, NSObjectProtocol, NSNumber, NSString};

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MVAvFoundationCaptureDelegate"]
    #[ivars = (RefCell<Arc<Mutex<Option<Frame>>>>, RefCell<Arc<Mutex<SourceStats>>>, Cell<u64>)]
    struct CaptureDelegate;

    impl CaptureDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars((
                RefCell::new(Arc::new(Mutex::new(None))),
                RefCell::new(Arc::new(Mutex::new(SourceStats::new()))),
                Cell::new(0),
            ));
            unsafe { msg_send![super(this), init] }
        }
    }

    unsafe impl NSObjectProtocol for CaptureDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CaptureDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
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
);

impl CaptureDelegate {
    fn new(slot: Arc<Mutex<Option<Frame>>>, stats: Arc<Mutex<SourceStats>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars((
            RefCell::new(slot),
            RefCell::new(stats),
            Cell::new(0),
        ));
        let this: Option<Retained<Self>> = unsafe { msg_send![super(this), init] };
        this.expect("AVFoundation capture delegate init failed")
    }
}


#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AvFoundationSourceConfig {
    pub device_unique_id: String,
}

/// Active AVFoundation capture source on its own thread.
pub struct AvFoundationSource {
    name: String,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AvFoundationSource {
    pub fn spawn(id: String, cfg: &AvFoundationSourceConfig) -> Self {
        let slot = Arc::new(Mutex::new(None::<Frame>));
        let stats = Arc::new(Mutex::new(SourceStats::new()));

        let slot2 = slot.clone();
        let stats2 = stats.clone();
        let device_unique_id = cfg.device_unique_id.clone();
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let running2 = running.clone();

        let thread = std::thread::Builder::new()
            .name(format!("avf-in-{id}"))
            .spawn(move || {
                Self::run_capture(device_unique_id, slot2, stats2, running2);
            })
            .expect("spawn avfoundation capture thread");

        Self {
            name: id,
            slot,
            stats,
            running,
            thread: Some(thread),
        }
    }

    fn run_capture(
        device_unique_id: String,
        slot: Arc<Mutex<Option<Frame>>>,
        stats: Arc<Mutex<SourceStats>>,
        running: Arc<std::sync::atomic::AtomicBool>,
    ) {
        let unique_id = NSString::from_str(&device_unique_id);
        let Some(device) = AVCaptureDevice::device_with_unique_id(&unique_id) else {
            tracing::error!(
                "AVFoundation source {}: device not found",
                device_unique_id
            );
            return;
        };

        let session = AVCaptureSession::new();
        let input = match AVCaptureDeviceInput::from_device(&device) {
            Ok(i) => i,
            Err(e) => {
                tracing::error!(
                    "AVFoundation source {}: could not create device input: {e:?}",
                    device_unique_id
                );
                return;
            }
        };

        let output = AVCaptureVideoDataOutput::new();
        output.set_always_discards_late_video_frames(true);

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
        let queue = DispatchQueue::new("com.multiviewer.avfoundation", DispatchQueueAttr::SERIAL);
        output.set_sample_buffer_delegate(delegate_obj, &queue);

        session.begin_configuration();
        session.add_input(&input);
        session.add_output(&output);
        session.commit_configuration();

        let _delegate = delegate;
        session.start_running();

        while running.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }

        session.stop_running();
    }
}

impl Drop for AvFoundationSource {
    fn drop(&mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() && let Err(e) = t.join() {
            tracing::error!("AVFoundation source {} thread join failed: {e:?}", self.name);
        }
    }
}

impl VideoSource for AvFoundationSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        self.slot.lock().unwrap().clone()
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        self.stats.clone()
    }
}
