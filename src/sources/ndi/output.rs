use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;
use serde::{Deserialize, Serialize};
use grafton_ndi::{NDI, Sender, SenderOptions, BorrowedVideoFrame};

use super::super::{OutputStats, Protocol};
use super::super::output::{OutputId, OutputLiveness, ReadbackPool, VideoOutput};

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
    stats: Arc<Mutex<OutputStats>>,
    frame_tx: Option<mpsc::SyncSender<Vec<u8>>>,
    liveness: OutputLiveness,
    readback: Mutex<ReadbackPool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl NdiOutput {
    pub fn new(id: OutputId, name: String, config: NdiOutputConfig, enabled: bool) -> Self {
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let stats = Arc::new(Mutex::new(OutputStats::default()));
        let stats_clone = Arc::clone(&stats);
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
                        grafton_ndi::PixelFormat::BGRX,
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
            stats,
            frame_tx: Some(frame_tx),
            liveness,
            readback: Mutex::new(ReadbackPool::new()),
            thread: Some(thread),
        }
    }
}

impl VideoOutput for NdiOutput {
    fn present(&self, texture: &wgpu::Texture, width: u32, height: u32, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        if !self.enabled.load(Ordering::Relaxed) {
            return false;
        }
        self.liveness.check_stall(&self.name);

        let aligned_bytes_per_row = (width * 4 + 255) & !255;
        let buffer_size = (aligned_bytes_per_row * height) as u64 + 8;
        let mut readback = self.readback.lock().unwrap();
        readback.ensure(device, "ndi-output-readback", buffer_size);
        let Some(slot) = readback.free() else {
            return false;
        };

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
                    buffer: readback.buffer(slot),
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

        readback.map(slot, width, height);
        return true;
    }

    fn finish(&self) {
        let mut readback = self.readback.lock().unwrap();
        let Some(slot) = readback.take_ready() else {
            return;
        };
        // The readback stride is padded to `COPY_BYTES_PER_ROW_ALIGNMENT`, so strip the padding.
        let (width, height) = readback.dims(slot);
        let row = (width * 4) as usize;
        let aligned = (row + 255) & !255;
        // The first 8 bytes carry width/height so the thread does not need to wait on atomics.
        let mut buffer = Vec::with_capacity(8 + row * height as usize);
        let copied = {
            let readback_buffer = readback.buffer(slot);
            match readback_buffer.slice(..).get_mapped_range() {
                Ok(view) => {
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
                    true
                }
                Err(e) => {
                    tracing::error!(output = self.name, "NDI output mapped range unavailable: {e:?}");
                    readback_buffer.unmap();
                    false
                }
            }
        };
        readback.release(slot);
        if !copied {
            return;
        }

        buffer[0..4].copy_from_slice(&width.to_le_bytes());
        buffer[4..8].copy_from_slice(&height.to_le_bytes());

        let Some(tx) = self.frame_tx.as_ref() else {
            return;
        };
        self.liveness.mark_sent();
        if tx.try_send(buffer).is_err() {
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
