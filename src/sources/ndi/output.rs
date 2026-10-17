use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;
use serde::{Deserialize, Serialize};
use grafton_ndi::{NDI, Sender, SenderOptions, BorrowedVideoFrame};

use super::super::{OutputStats, Protocol};
use super::super::output::{OutputId, OutputLiveness, VideoOutput};

// @docs protocol="NDI" direction=output
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct NdiOutputConfig {
    // @docs hidden
    /// Name advertised on the network. Empty uses the output's name.
    pub sender_name: String,
}

pub struct NdiOutput {
    name: String,
    enabled: AtomicBool,
    width: AtomicU32,
    height: AtomicU32,
    stats: Arc<Mutex<OutputStats>>,
    frame_tx: Option<mpsc::SyncSender<Vec<u8>>>,
    pending: Arc<AtomicBool>,
    liveness: OutputLiveness,
    readback: Mutex<Option<wgpu::Buffer>>,
    pending_readback: Mutex<Option<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl NdiOutput {
    pub fn new(id: OutputId, name: String, config: NdiOutputConfig, enabled: bool) -> Self {
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let stats = Arc::new(Mutex::new(OutputStats::default()));
        let stats_clone = Arc::clone(&stats);
        let pending = Arc::new(AtomicBool::new(false));
        let pending_t = Arc::clone(&pending);
        let liveness = OutputLiveness::default();
        let worker_flag = liveness.exit_flag();

        let thread_name = name.clone();
        let thread = thread::Builder::new()
            .name(format!("ndi-out-{id}"))
            .spawn(move || {
                let _alive_guard = worker_flag;
                let ndi = match NDI::new() {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!(output=thread_name, "NDI init failed: {}", e);
                        return;
                    }
                };
                let options = SenderOptions::builder(&config.sender_name)
                    .clock_video(true)
                    .build();
                let mut sender = match Sender::new(&ndi, &options) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(output=thread_name, "NDI Sender creation failed: {}", e);
                        return;
                    }
                };

                while let Ok(buffer) = frame_rx.recv() {
                    pending_t.store(false, Ordering::Release);
                    if buffer.len() < 16 {
                        continue;
                    }
                    let frame_w = u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
                    let frame_h = u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
                    let expected = (frame_w * frame_h * 4) as usize;
                    if buffer.len() < expected + 8 {
                        continue;
                    }

                    let frame = match BorrowedVideoFrame::try_from_uncompressed(
                        &buffer[8..],
                        frame_w as i32,
                        frame_h as i32,
                        grafton_ndi::PixelFormat::BGRA,
                        60,
                        1,
                    ) {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::error!(output=thread_name, "NDI output frame build failed: {}", e);
                            continue;
                        }
                    };

                    let send_span = tracing::debug_span!("ndi_send");
                    let _send_guard = send_span.entered();
                    let start = Instant::now();
                    // The returned async token borrows the buffer, so we cannot store it across
                    // loop iterations. Dropping it here flushes the frame before the next send.
                    let _token = sender.send_video_async(&frame);
                    {
                        let mut s = stats_clone.lock().unwrap();
                        s.width = frame_w;
                        s.height = frame_h;
                        s.record_sent();
                        s.send_time_ms = start.elapsed().as_secs_f32() * 1000.0;
                    }
                }

                sender.flush_async_blocking();
            })
            .expect("spawn ndi output thread");

        Self {
            name,
            enabled: AtomicBool::new(enabled),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            stats,
            frame_tx: Some(frame_tx),
            pending,
            liveness,
            readback: Mutex::new(None),
            pending_readback: Mutex::new(None),
            thread: Some(thread),
        }
    }
}

impl VideoOutput for NdiOutput {
    fn present(&self, texture: &wgpu::Texture, width: u32, height: u32, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        if !self.enabled.load(Ordering::Relaxed) {
            return false;
        }
        if self.busy() {
            self.liveness.check_stall(&self.name);
            return false;
        }

        self.width.store(width, Ordering::Relaxed);
        self.height.store(height, Ordering::Relaxed);

        let aligned_bytes_per_row = (width * 4 + 255) & !255;
        let buffer_size = (aligned_bytes_per_row * height) as u64;
        let mut readback_slot = self.readback.lock().unwrap();
        if readback_slot.as_ref().is_none_or(|b| b.size() != buffer_size + 8) {
            *readback_slot = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ndi-output-readback"),
                size: buffer_size + 8,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));
        }
        let readback_buffer = readback_slot.as_ref().unwrap();

        {
            let readback_span = tracing::debug_span!("readback");
            let _readback_guard = readback_span.entered();
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ndi-output-readback"),
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: readback_buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 8,
                        bytes_per_row: Some(aligned_bytes_per_row),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit(Some(encoder.finish()));
        }

        let (tx, rx) = mpsc::channel::<Result<(), wgpu::BufferAsyncError>>();
        readback_buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        *self.pending_readback.lock().unwrap() = Some(rx);
        return true;
    }

    fn finish(&self) {
        let Some(rx) = self.pending_readback.lock().unwrap().take() else {
            return;
        };
        let mapped = match rx.try_recv() {
            Ok(result) => result,
            Err(_) => {
                // The map never completed (poll error / device lost). Drop the
                // buffer so the next frame recreates it instead of re-mapping a
                // buffer that is still pending.
                *self.readback.lock().unwrap() = None;
                tracing::error!(output=self.name, "NDI output readback did not complete");
                return;
            }
        };
        if mapped.is_err() {
            tracing::error!(output=self.name, "NDI output readback map failed");
            return;
        }

        let readback_slot = self.readback.lock().unwrap();
        let Some(readback_buffer) = readback_slot.as_ref() else {
            return;
        };
        // The readback stride is padded to `COPY_BYTES_PER_ROW_ALIGNMENT`, so strip the padding.
        let width = self.width.load(Ordering::Relaxed);
        let height = self.height.load(Ordering::Relaxed);
        let row = (width * 4) as usize;
        let aligned = (row + 255) & !255;
        let view = match readback_buffer.slice(..).get_mapped_range() {
            Ok(view) => view,
            Err(e) => {
                tracing::error!(output = self.name, "NDI output mapped range unavailable: {e:?}");
                return;
            }
        };
        // The first 8 bytes carry width/height so the thread does not need to wait on atomics.
        let mut buffer = Vec::with_capacity(8 + row * height as usize);
        buffer.extend_from_slice(&[0u8; 8]);
        if aligned == row {
            buffer.extend_from_slice(&view[8..8 + row * height as usize]);
        } else {
            for y in 0..height as usize {
                let src_start = 8 + y * aligned;
                buffer.extend_from_slice(&view[src_start..src_start + row]);
            }
        }
        drop(view);
        readback_buffer.unmap();

        buffer[0..4].copy_from_slice(&width.to_le_bytes());
        buffer[4..8].copy_from_slice(&height.to_le_bytes());

        let Some(tx) = self.frame_tx.as_ref() else {
            return;
        };
        // Publish in-flight before the handoff: the worker clears `pending` right
        // after recv(), so storing it after try_send can be overwritten and wedge
        // busy() true forever.
        self.pending.store(true, Ordering::Release);
        self.liveness.mark_sent();
        if tx.try_send(buffer).is_err() {
            self.pending.store(false, Ordering::Release);
            self.liveness.clear_sent();
            self.stats.lock().unwrap().frames_dropped += 1;
        }
    }

    fn name(&self) -> &str {
        return &self.name;
    }

    fn enabled(&self) -> bool {
        return self.enabled.load(Ordering::Relaxed);
    }

    fn busy(&self) -> bool {
        return self.pending.load(Ordering::Acquire) && self.liveness.is_alive();
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn stats(&self) -> Arc<Mutex<OutputStats>> {
        return self.stats.clone();
    }

    fn protocol(&self) -> Protocol {
        return Protocol::Ndi;
    }
}

impl Drop for NdiOutput {
    fn drop(&mut self) {
        drop(self.frame_tx.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
