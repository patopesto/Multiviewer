use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize, Deserializer, Serializer};
use grafton_ndi::{NDI, Receiver, ReceiverOptions, LineStrideOrSize};

use super::{NdiReceiverBandwidth, NdiReceiverColorFormat, NdiSourceInfo};
use super::super::{CpuFrame, Frame, FramePool, PixelFormat, SourceRef, SourceStats, VideoSource};

#[derive(Clone, Debug, PartialEq)]
pub struct NdiSourceConfig {
    pub bandwidth: NdiReceiverBandwidth,
    pub color_format: NdiReceiverColorFormat,
}

impl Serialize for NdiSourceConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("NdiSourceConfig", 2)?;
        let bandwidth_str = match self.bandwidth {
            NdiReceiverBandwidth::Highest => "Highest",
            _ => "Lowest",
        };
        let color_format_str = match self.color_format {
            NdiReceiverColorFormat::BGRX_BGRA => "BGRX_BGRA",
            NdiReceiverColorFormat::UYVY_BGRA => "UYVY_BGRA",
            NdiReceiverColorFormat::RGBX_RGBA => "RGBX_RGBA",
            NdiReceiverColorFormat::UYVY_RGBA => "UYVY_RGBA",
            NdiReceiverColorFormat::Fastest => "Fastest",
            NdiReceiverColorFormat::Best => "Best",
            _ => "UYVY_RGBA",
        };
        state.serialize_field("bandwidth", bandwidth_str)?;
        state.serialize_field("color_format", color_format_str)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for NdiSourceConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            bandwidth: String,
            color_format: String,
        }
        let helper = Helper::deserialize(deserializer)?;
        let bandwidth = match helper.bandwidth.as_str() {
            "Highest" => NdiReceiverBandwidth::Highest,
            _ => NdiReceiverBandwidth::Lowest,
        };
        let color_format = match helper.color_format.as_str() {
            "BGRX_BGRA" => NdiReceiverColorFormat::BGRX_BGRA,
            "UYVY_BGRA" => NdiReceiverColorFormat::UYVY_BGRA,
            "RGBX_RGBA" => NdiReceiverColorFormat::RGBX_RGBA,
            "UYVY_RGBA" => NdiReceiverColorFormat::UYVY_RGBA,
            "Fastest" => NdiReceiverColorFormat::Fastest,
            "Best" => NdiReceiverColorFormat::Best,
            _ => NdiReceiverColorFormat::UYVY_RGBA,
        };
        Ok(NdiSourceConfig { bandwidth, color_format })
    }
}

impl Default for NdiSourceConfig {
    fn default() -> Self {
        Self {
            bandwidth: NdiReceiverBandwidth::Lowest,
            // Request UYVY for lower bandwidth; the SDK falls back to RGBA for alpha sources.
            color_format: NdiReceiverColorFormat::UYVY_RGBA,
        }
    }
}

/// Active NDI receiver on its own thread.
pub struct NdiSource {
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    source_ref: SourceRef,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl NdiSource {
    pub fn spawn(source_ref: SourceRef, source: NdiSourceInfo, cfg: &NdiSourceConfig) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let running = Arc::new(AtomicBool::new(true));

        let slot2 = slot.clone();
        let stats2 = stats.clone();
        let running2 = running.clone();
        let trace_ref = source_ref.clone();
        let bandwidth = cfg.bandwidth;
        let color_format = cfg.color_format;

        let thread = std::thread::Builder::new()
            .name(format!("ndi-in-{source_ref}"))
            .spawn(move || {
                run_capture(source, bandwidth, color_format, slot2, stats2, running2, trace_ref);
            })
            .expect("spawn ndi-recv");
        Self { slot, stats, source_ref, running, thread: Some(thread) }
    }
}

/// Receive and publish frames on the source's thread until `running` clears.
#[allow(clippy::too_many_arguments)]
fn run_capture(
    source: NdiSourceInfo,
    bandwidth: NdiReceiverBandwidth,
    color_format: NdiReceiverColorFormat,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    source_ref: SourceRef,
) {
    let ndi = match NDI::new() {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(source=source_ref, "NDI init failed: {}", e);
            return;
        }
    };
    let options = ReceiverOptions::builder(source)
        .color(color_format)
        .bandwidth(bandwidth)
        .build();
    let receiver = match Receiver::new(&ndi, &options) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(source=source_ref, "NDI Receiver creation failed: {}", e);
            return;
        }
    };
    let mut seq = 0u64;
    let mut warned_formats: HashSet<u32> = HashSet::new();
    let mut pool = FramePool::new();
    while running.load(Ordering::Relaxed) {
        let frame_span = tracing::debug_span!("ndi_frame");
        let _frame_guard = frame_span.entered();
        // Zero-copy poll: the SDK buffer is borrowed and copied straight into a pooled frame buffer
        match receiver.video().try_capture_ref(Duration::from_millis(100)) {
            Ok(Some(frame)) => {
                let w = frame.width() as u32;
                let h = frame.height() as u32;
                let (fmt, bpp) = match frame.pixel_format() {
                    grafton_ndi::PixelFormat::UYVY => (PixelFormat::Uyvy422, 2usize),
                    grafton_ndi::PixelFormat::RGBA | grafton_ndi::PixelFormat::RGBX => {
                        (PixelFormat::Rgba8, 4usize)
                    }
                    grafton_ndi::PixelFormat::BGRA | grafton_ndi::PixelFormat::BGRX => {
                        (PixelFormat::Bgra8, 4usize)
                    }
                    pf => {
                        if warned_formats.insert(pf as u32) {
                            tracing::warn!(source=source_ref, "NDI Unsupported pixel format {pf:?}, frame dropped");
                        }
                        continue;
                    }
                };
                let stride = match frame.line_stride_or_size() {
                    LineStrideOrSize::LineStrideBytes(s) => s as usize,
                    _ => (w as usize) * bpp,
                };
                let t0 = Instant::now();
                let src = frame.data();
                let mut data = pool.take(src.len());
                data.copy_from_slice(src);
                let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;
                {
                    let mut s = stats.lock().unwrap();
                    s.record_frame(w, h, fmt.label(), 0.0);
                    s.record_copy_time(copy_ms);
                }
                let old = slot.lock().unwrap().replace(Frame::Cpu(CpuFrame {
                    data: Arc::new(data),
                    w,
                    h,
                    fmt,
                    pitch: stride as u32,
                    seq,
                }));
                pool.give(old);
                seq += 1;
            }
            // No frame within the timeout, wait
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                tracing::warn!(source=source_ref, "NDI capture timeout: {}", e);
            }
        }
    }
}

impl Drop for NdiSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take()
            && let Err(e) = t.join()
        {
            tracing::error!(source=self.source_ref, "Thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for NdiSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        return self.slot.lock().unwrap().clone();
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }
}
