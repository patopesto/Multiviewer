use super::super::{CpuFrame, Frame, PixelFormat, SourceStats, VideoSource};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct NdiSourceConfig {
    pub bandwidth: grafton_ndi::ReceiverBandwidth,
    pub color_format: grafton_ndi::ReceiverColorFormat,
}

impl Default for NdiSourceConfig {
    fn default() -> Self {
        Self {
            bandwidth: grafton_ndi::ReceiverBandwidth::Lowest,
            // Request UYVY for lower bandwidth; the SDK falls back to RGBA for alpha sources.
            color_format: grafton_ndi::ReceiverColorFormat::UYVY_RGBA,
        }
    }
}

/// Active NDI receiver on its own thread.
pub struct NdiSource {
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    #[allow(dead_code)]
    name: String,
}

impl NdiSource {
    pub fn spawn(name: String, source: grafton_ndi::Source, cfg: &NdiSourceConfig) -> Self {
        use grafton_ndi::{LineStrideOrSize, NDI, Receiver, ReceiverOptions};
        let slot = Arc::new(Mutex::new(None));
        let slot2 = slot.clone();
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let stats2 = stats.clone();
        let thread_name = name.clone();
        let bandwidth = cfg.bandwidth;
        let color_format = cfg.color_format;
        std::thread::Builder::new()
            .name(format!("ndi-recv-{thread_name}"))
            .spawn(move || {
                let ndi = match NDI::new() {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!("NDI init failed for {thread_name}: {e}");
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
                        tracing::error!("NDI Receiver failed for {thread_name}: {e}");
                        return;
                    }
                };
                let mut seq = 0u64;
                let mut warned_formats: HashSet<u32> = HashSet::new();
                loop {
                    match receiver.video().capture(Duration::from_millis(500)) {
                        Ok(frame) => {
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
                                        tracing::warn!(
                                            "NDI source {thread_name}: unsupported pixel format {pf:?}, frame dropped"
                                        );
                                    }
                                    continue;
                                }
                            };
                            let stride = match frame.line_stride_or_size() {
                                LineStrideOrSize::LineStrideBytes(s) => s as usize,
                                _ => (w as usize) * bpp,
                            };
                            let expected = (w as usize) * (h as usize) * bpp;
                            let t0 = Instant::now();
                            let data = if stride == (w as usize) * bpp {
                                frame.data().to_vec()
                            } else {
                                let mut packed = vec![0u8; expected];
                                for y in 0..h as usize {
                                    let src = y * stride;
                                    let dst = y * (w as usize) * bpp;
                                    let row_bytes = (w as usize) * bpp;
                                    packed[dst..dst + row_bytes]
                                        .copy_from_slice(&frame.data()[src..src + row_bytes]);
                                }
                                packed
                            };
                            let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;
                            {
                                let mut s = stats2.lock().unwrap();
                                s.record_frame(w, h, fmt.label(), 0.0);
                                s.record_copy_time(copy_ms);
                            }
                            *slot2.lock().unwrap() = Some(Frame::Cpu(CpuFrame {
                                data: Arc::new(data),
                                w,
                                h,
                                fmt,
                                seq,
                            }));
                            seq += 1;
                        }
                        Err(e) => {
                            tracing::trace!("NDI capture timeout for {thread_name}: {e}");
                        }
                    }
                }
            })
            .expect("spawn ndi-recv");
        Self { slot, stats, name }
    }
}

impl VideoSource for NdiSource {
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
