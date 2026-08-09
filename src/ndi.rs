use grafton_ndi::{
    Finder, FinderOptions, LineStrideOrSize, NDI, Receiver, ReceiverBandwidth,
    ReceiverColorFormat, ReceiverOptions, Source,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Background NDI discovery thread.
pub struct Discovery {
    sources: Arc<Mutex<Vec<Source>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let sources = Arc::new(Mutex::new(Vec::new()));
        let sources2 = sources.clone();
        std::thread::Builder::new()
            .name("ndi-discovery".into())
            .spawn(move || {
                let ndi = match NDI::new() {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!("NDI init failed in discovery: {e}");
                        return;
                    }
                };
                let finder = match Finder::new(
                    &ndi,
                    &FinderOptions::builder().show_local_sources(true).build(),
                ) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::error!("NDI Finder failed: {e}");
                        return;
                    }
                };
                loop {
                    match finder.current_sources() {
                        Ok(list) => {
                            let mut lock = sources2.lock().unwrap();
                            *lock = list;
                        }
                        Err(e) => tracing::warn!("NDI discovery error: {e}"),
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn ndi-discovery");
        Self { sources }
    }

    pub fn list(&self) -> Vec<Source> {
        self.sources.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    pub fn find_by_name(&self, name: &str) -> Option<Source> {
        self.sources.lock().unwrap().iter().find(|s| s.name == name).cloned()
    }
}

/// Active NDI receiver on its own thread.
pub struct NdiSource {
    slot: Arc<Mutex<Option<crate::source::Frame>>>,
    name: String,
}

impl NdiSource {
    pub fn spawn(name: String, source: Source) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let slot2 = slot.clone();
        let thread_name = name.clone();
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
                    .color(ReceiverColorFormat::RGBX_RGBA)
                    .bandwidth(ReceiverBandwidth::Lowest)
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
                            *slot2.lock().unwrap() = Some(crate::source::Frame::Cpu(
                                crate::source::CpuFrame {
                                    data: Arc::new(data),
                                    w,
                                    h,
                                    fmt: crate::source::PixelFormat::Rgba8,
                                    seq,
                                },
                            ));
                            seq += 1;
                        }
                        Err(e) => {
                            // Timeouts are normal when source goes offline
                            tracing::trace!("NDI capture timeout for {thread_name}: {e}");
                        }
                    }
                }
            })
            .expect("spawn ndi-recv");
        Self { slot, name }
    }
}

impl crate::source::VideoSource for NdiSource {
    fn latest(&self) -> Option<crate::source::Frame> {
        self.slot.lock().unwrap().clone()
    }

    fn name(&self) -> &str {
        &self.name
    }
}
