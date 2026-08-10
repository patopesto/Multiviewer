use super::super::{CpuFrame, Frame, PixelFormat, VideoSource};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct NdiConfig {
    pub bandwidth: grafton_ndi::ReceiverBandwidth,
    pub color_format: grafton_ndi::ReceiverColorFormat,
}

impl Default for NdiConfig {
    fn default() -> Self {
        Self {
            bandwidth: grafton_ndi::ReceiverBandwidth::Lowest,
            color_format: grafton_ndi::ReceiverColorFormat::RGBX_RGBA,
        }
    }
}

/// Active NDI receiver on its own thread.
pub struct NdiSource {
    slot: Arc<Mutex<Option<Frame>>>,
    #[allow(dead_code)]
    name: String,
}

impl NdiSource {
    pub fn spawn(name: String, source: grafton_ndi::Source, cfg: &NdiConfig) -> Self {
        use grafton_ndi::{
            LineStrideOrSize, NDI, Receiver, ReceiverOptions,
        };
        let slot = Arc::new(Mutex::new(None));
        let slot2 = slot.clone();
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
                loop {
                    match receiver.video().capture(Duration::from_millis(500)) {
                        Ok(frame) => {
                            let w = frame.width() as u32;
                            let h = frame.height() as u32;
                            let stride = match frame.line_stride_or_size() {
                                LineStrideOrSize::LineStrideBytes(s) => s as usize,
                                _ => (w * 4) as usize,
                            };
                            let expected = (w * h * 4) as usize;
                            let data = if stride == w as usize * 4 {
                                frame.data().to_vec()
                            } else {
                                let mut packed = vec![0u8; expected];
                                for y in 0..h as usize {
                                    let src = y * stride;
                                    let dst = y * (w as usize * 4);
                                    packed[dst..dst + (w as usize * 4)]
                                        .copy_from_slice(&frame.data()[src..src + (w as usize * 4)]);
                                }
                                packed
                            };
                            *slot2.lock().unwrap() = Some(Frame::Cpu(CpuFrame {
                                data: Arc::new(data),
                                w,
                                h,
                                fmt: PixelFormat::Rgba8,
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
        Self { slot, name }
    }
}

impl VideoSource for NdiSource {
    fn latest(&self) -> Option<Frame> {
        self.slot.lock().unwrap().clone()
    }

    fn name(&self) -> &str {
        &self.name
    }
}
