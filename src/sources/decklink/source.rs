use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use multiviewer_decklink::{DecklinkPixelFormat, VideoConnection};
use multiviewer_decklink::{decklink_source_new, decklink_source_free, decklink_source_set_connection, decklink_source_start, decklink_source_stop, decklink_source_poll_frame};

use crate::sources::{CpuFrame, Frame, PixelFormat, SourceRef, SourceStats, VideoSource};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DecklinkSourceConfig {
    // Unspecified falls back to whatever the device reports first.
    #[serde(default)]
    pub connection: VideoConnection,
}

pub struct DecklinkSource {
    source_ref: SourceRef,
    latest: Arc<Mutex<Option<CpuFrame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DecklinkSource {
    pub fn spawn(source_ref: SourceRef, cfg: &DecklinkSourceConfig) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let latest2 = latest.clone();
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let stats2 = stats.clone();
        let running = Arc::new(AtomicBool::new(true));
        let running2 = running.clone();
        let device_name = source_ref.clone();
        let trace_ref = source_ref.clone();
        let connection = cfg.connection;
        let thread = std::thread::Builder::new()
            .name(format!("decklink-in-{source_ref}"))
            .spawn(move || {
                unsafe {
                    let display_name_c = match CString::new(device_name) {
                        Ok(c) => c,
                        Err(_) => {
                            tracing::error!(source=trace_ref, "DeckLink display name contains null");
                            return;
                        }
                    };
                    let src = decklink_source_new(display_name_c.as_ptr());
                    if src.is_null() {
                        tracing::error!(source=trace_ref, "DeckLink source creation failed");
                        return;
                    }
                    if !matches!(connection, VideoConnection::Unspecified) {
                        decklink_source_set_connection(src, connection as u32);
                    }
                    if !decklink_source_start(src) {
                        tracing::error!(source=trace_ref, "DeckLink source start failed");
                        decklink_source_free(src);
                        return;
                    }
                    // Phase 1: pre-allocated buffer pool — avoid per-frame alloc/copy.
                    const MAX_SIZE: usize = 3840 * 2160 * 4;
                    let mut pool: Vec<Vec<u8>> = vec![vec![0u8; MAX_SIZE]; 3];
                    let mut slot = 0usize;
                    let mut last_seq = 0u64;
                    while running2.load(Ordering::Relaxed) {
                        if pool[slot].len() < MAX_SIZE {
                            pool[slot].resize(MAX_SIZE, 0);
                        }
                        let mut w = 0;
                        let mut h = 0;
                        let mut seq = 0u64;
                        let mut fmt = 0u32;
                        let mut nominal_fps = 0.0f64;
                        let t0 = Instant::now();
                        let got = decklink_source_poll_frame(
                            src,
                            pool[slot].as_mut_ptr(),
                            pool[slot].len(),
                            &mut w,
                            &mut h,
                            &mut seq,
                            &mut fmt,
                            &mut nominal_fps,
                        );
                        // Only process and count a frame when the C++ callback has
                        // published a new seq. The 5 ms poll otherwise returns the
                        // same front buffer repeatedly.
                        if got && seq != last_seq {
                            let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;
                            let pixel_format = match DecklinkPixelFormat::try_from(fmt) {
                                Ok(DecklinkPixelFormat::Bgra8) => PixelFormat::Bgra8,
                                Ok(DecklinkPixelFormat::Uyvy422) => PixelFormat::Uyvy422,
                                _ => PixelFormat::Rgba8,
                            };
                            let bpp = match pixel_format {
                                PixelFormat::Uyvy422 => 2,
                                _ => 4,
                            };
                            let data_size = (w * h * bpp) as usize;
                            pool[slot].truncate(data_size);

                            {
                                let mut s = stats2.lock().unwrap();
                                s.record_frame(
                                    w as u32,
                                    h as u32,
                                    pixel_format.label(),
                                    nominal_fps,
                                );
                                s.record_copy_time(copy_ms);
                                if last_seq != 0 && seq > last_seq + 1 {
                                    s.record_dropped(seq - last_seq - 1);
                                }
                            }
                            last_seq = seq;

                            let mut guard = latest2.lock().unwrap();
                            let old = guard.take();
                            *guard = Some(CpuFrame {
                                data: Arc::new(std::mem::take(&mut pool[slot])),
                                w: w as u32,
                                h: h as u32,
                                fmt: pixel_format,
                                pitch: 0,
                                seq,
                            });
                            drop(guard);

                            // Try to reclaim the old frame's buffer back into the pool.
                            if let Some(old_frame) = old
                                && let Ok(mut vec) = Arc::try_unwrap(old_frame.data)
                            {
                                vec.resize(MAX_SIZE, 0);
                                pool[slot] = vec;
                            }
                            slot = (slot + 1) % pool.len();
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    decklink_source_stop(src);
                    decklink_source_free(src);
                }
            })
            .expect("spawn decklink source");
        Self {
            source_ref,
            latest,
            stats,
            running,
            thread: Some(thread),
        }
    }
}

impl Drop for DecklinkSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take()
            && let Err(e) = t.join()
        {
            tracing::error!(source=self.source_ref, "DeckLink thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for DecklinkSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        self.latest
            .lock()
            .unwrap()
            .as_ref()
            .map(|f| Frame::Cpu(f.clone()))
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        self.stats.clone()
    }
}
