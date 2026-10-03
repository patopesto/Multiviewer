use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;
use serde::{Deserialize, Serialize};
use grafton_ndi::{NDI, Sender, SenderOptions, BorrowedVideoFrame};

use super::super::Protocol;
use super::super::output::{OutputId, OutputStats, VideoOutput};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct NdiOutputConfig {
    #[serde(default)]
    pub sender_name: String,
}

pub struct NdiOutput {
    #[allow(dead_code)]
    id: OutputId,
    name: String,
    enabled: AtomicBool,
    width: AtomicU32,
    height: AtomicU32,
    stats: Arc<Mutex<OutputStats>>,
    frame_tx: mpsc::SyncSender<Vec<u8>>,
    pending: Arc<AtomicBool>,
    readback: Mutex<Option<wgpu::Buffer>>,
    #[allow(dead_code)]
    thread: Option<thread::JoinHandle<()>>,
}

impl NdiOutput {
    pub fn new(id: OutputId, name: String, config: NdiOutputConfig, enabled: bool) -> Self {
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let stats = Arc::new(Mutex::new(OutputStats::default()));
        let stats_clone = Arc::clone(&stats);
        let pending = Arc::new(AtomicBool::new(false));
        let pending_t = Arc::clone(&pending);

        let thread_name = name.clone();
        let thread = thread::Builder::new()
            .name(format!("ndi-out-{id}"))
            .spawn(move || {
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
                        s.frames_sent += 1;
                        s.send_time_ms = start.elapsed().as_secs_f32() * 1000.0;
                    }
                }

                sender.flush_async_blocking();
            })
            .expect("spawn ndi output thread");

        Self {
            id,
            name,
            enabled: AtomicBool::new(enabled),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            stats,
            frame_tx,
            pending,
            readback: Mutex::new(None),
            thread: Some(thread),
        }
    }
}

impl VideoOutput for NdiOutput {
    fn present(&self, texture: &wgpu::Texture, _width: u32, _height: u32,device: &wgpu::Device, queue: &wgpu::Queue) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        if self.pending.load(Ordering::Acquire) {
            return;
        }

        let width = texture.width();
        let height = texture.height();
        self.width.store(width, Ordering::Relaxed);
        self.height.store(height, Ordering::Relaxed);

        let bytes_per_row = width * 4;
        let buffer_size = (bytes_per_row * height) as u64;
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
                        bytes_per_row: Some(bytes_per_row),
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

        let wait_span = tracing::debug_span!("readback_wait");
        let _wait_guard = wait_span.entered();
        let buffer_slice = readback_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), wgpu::BufferAsyncError>>();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
            tracing::error!(output=self.name, "NDI output poll failed");
            return;
        }
        if rx.recv().unwrap().is_err() {
            tracing::error!(output=self.name, "NDI output readback map failed");
            return;
        }

        let view = buffer_slice.get_mapped_range();
        let mut buffer = Vec::with_capacity(view.len());
        buffer.extend_from_slice(&view);
        drop(view);
        readback_buffer.unmap();

        // Embed width/height in the first 8 bytes so the thread does not need to wait on atomics.
        buffer[0..4].copy_from_slice(&width.to_le_bytes());
        buffer[4..8].copy_from_slice(&height.to_le_bytes());

        if self.frame_tx.try_send(buffer).is_ok() {
            self.pending.store(true, Ordering::Release);
        } else {
            let mut s = self.stats.lock().unwrap();
            s.frames_dropped += 1;
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn busy(&self) -> bool {
        return self.pending.load(Ordering::Acquire);
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn stats(&self) -> Arc<Mutex<OutputStats>> {
        Arc::clone(&self.stats)
    }

    fn protocol(&self) -> Protocol {
        Protocol::Ndi
    }
}
