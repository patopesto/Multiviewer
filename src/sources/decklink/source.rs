use crate::sources::{CpuFrame, Frame, PixelFormat, SourceStats, VideoSource};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Values must match DecklinkPixelFormatOut in decklink_shim.cpp.
#[allow(dead_code)]
const DECKLINK_FMT_RGBA8: i32 = 1;
const DECKLINK_FMT_BGRA8: i32 = 2;
const DECKLINK_FMT_UYVY422: i32 = 3;

pub enum DecklinkSourceHandle {}

#[derive(Clone)]
pub struct DecklinkConfig {
    pub connection: String,
    pub supported_connections: String,
}

impl Default for DecklinkConfig {
    fn default() -> Self {
        Self {
            connection: String::new(),
            supported_connections: String::new(),
        }
    }
}

pub struct DecklinkSource {
    name: String,
    latest: Arc<Mutex<Option<CpuFrame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DecklinkSource {
    pub fn spawn(name: String, display_name: String, cfg: &DecklinkConfig) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let latest2 = latest.clone();
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let stats2 = stats.clone();
        let running = Arc::new(AtomicBool::new(true));
        let running2 = running.clone();
        let name_for_thread = name.clone();
        let connection = cfg.connection.clone();
            let thread = std::thread::Builder::new()
            .name(format!("decklink-{}", name))
            .spawn(move || {
                unsafe {
                    let display_name_c = match CString::new(display_name) {
                        Ok(c) => c,
                        Err(_) => {
                            tracing::error!("DeckLink display name contains null for {}", name_for_thread);
                            return;
                        }
                    };
                    let src = decklink_source_new(display_name_c.as_ptr());
                    if src.is_null() {
                        tracing::error!("DeckLink source creation failed for {}", name_for_thread);
                        return;
                    }
                    if !connection.is_empty() {
                        let connection_c = match CString::new(connection) {
                            Ok(c) => c,
                            Err(_) => {
                                tracing::error!("DeckLink connection string contains null for {}", name_for_thread);
                                decklink_source_free(src);
                                return;
                            }
                        };
                        decklink_source_set_connection(src, connection_c.as_ptr());
                    }
                    if !decklink_source_start(src) {
                        tracing::error!("DeckLink source start failed for {}", name_for_thread);
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
                        let mut fmt = 0i32;
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
                            let (pixel_format, bpp) = if fmt == DECKLINK_FMT_BGRA8 {
                                (PixelFormat::Bgra8, 4)
                            } else if fmt == DECKLINK_FMT_UYVY422 {
                                (PixelFormat::Uyvy422, 2)
                            } else {
                                (PixelFormat::Rgba8, 4)
                            };
                            let data_size = (w * h * bpp) as usize;
                            pool[slot].truncate(data_size);

                            {
                                let mut s = stats2.lock().unwrap();
                                s.record_frame(w as u32, h as u32, pixel_format.label(), nominal_fps);
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
                                seq,
                            });
                            drop(guard);

                            // Try to reclaim the old frame's buffer back into the pool.
                            if let Some(old_frame) = old {
                                if let Ok(mut vec) = Arc::try_unwrap(old_frame.data) {
                                    vec.resize(MAX_SIZE, 0);
                                    pool[slot] = vec;
                                }
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
            name,
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
        if let Some(t) = self.thread.take() {
            if let Err(e) = t.join() {
                tracing::error!("DeckLink thread join failed for {}: {:?}", self.name, e);
            }
        }
    }
}

impl VideoSource for DecklinkSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        self.latest.lock().unwrap().as_ref().map(|f| Frame::Cpu(f.clone()))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        self.stats.clone()
    }
}

unsafe extern "C" {
    fn decklink_source_new(display_name: *const std::ffi::c_char) -> *mut DecklinkSourceHandle;
    fn decklink_source_free(s: *mut DecklinkSourceHandle);
    fn decklink_source_set_connection(s: *mut DecklinkSourceHandle, connection: *const std::ffi::c_char);
    fn decklink_source_start(s: *mut DecklinkSourceHandle) -> bool;
    fn decklink_source_stop(s: *mut DecklinkSourceHandle);
    fn decklink_source_poll_frame(
        s: *mut DecklinkSourceHandle,
        out_rgba: *mut u8,
        out_size: usize,
        w: *mut i32,
        h: *mut i32,
        seq: *mut u64,
        fmt_out: *mut i32,
        nominal_fps_out: *mut f64,
    ) -> bool;
}
