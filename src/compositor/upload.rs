use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::Compositor;

/// Persistent staging buffer for one source texture. Reusing it keeps the
/// allocation (and its page faults) out of the per-frame path; the mapping is
/// the fence: a buffer is writable only once its re-map has completed, which
/// wgpu schedules after the submission reading it has finished.
pub(super) struct Staging {
    buffer: wgpu::Buffer,
    size: u64,
    pitch: u32,
    rows: u32,
    width: u32,
    free: Arc<AtomicBool>,
}

impl Staging {
    pub(super) fn new(device: &wgpu::Device, label: &str, width: u32, row_bytes: u32, rows: u32) -> Self {
        let pitch = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let size = pitch as u64 * rows as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        return Self {
            buffer,
            size,
            pitch,
            rows,
            width,
            free: Arc::new(AtomicBool::new(true)),
        };
    }

    pub(super) fn is_free(&self) -> bool {
        return self.free.load(Ordering::Acquire);
    }

    /// Copy one frame into the mapping. Row by row when the source stride
    /// differs from the 256-aligned one `copy_buffer_to_texture` needs.
    pub(super) fn write(&self, data: &[u8], source_pitch: u32, row_bytes: u32) {
        let Ok(mut view) = self.buffer.slice(0..self.size).get_mapped_range_mut() else {
            tracing::warn!("staging buffer unavailable for writing");
            return;
        };
        if source_pitch == self.pitch {
            let len = row_bytes as usize * self.rows as usize;
            view.slice(..len).copy_from_slice(&data[..len]);
            return;
        }
        for row in 0..self.rows as usize {
            let src = row * source_pitch as usize;
            let dst = row * self.pitch as usize;
            view.slice(dst..dst + row_bytes as usize).copy_from_slice(&data[src..src + row_bytes as usize]);
        }
    }

    fn record_copy(&self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) {
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.pitch),
                    rows_per_image: Some(self.rows),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.rows,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// Upload encoder plus the map lifecycle that keeps the staging buffers
/// reusable: one submission per tick, one re-map per buffer per tick.
pub(super) struct Uploader {
    encoder: Option<wgpu::CommandEncoder>,
    recorded: bool,
    batch: Vec<Arc<Staging>>,
    inflight: Vec<Arc<Staging>>,
}

impl Uploader {
    pub(super) fn new() -> Self {
        return Self {
            encoder: None,
            recorded: false,
            batch: Vec::new(),
            inflight: Vec::new(),
        };
    }

    /// Drive the map callbacks that release buffers back to this tick. Only
    /// polls while something is in flight.
    pub(super) fn reap(&mut self, device: &wgpu::Device) {
        if self.inflight.is_empty() {
            return;
        }
        if device.poll(wgpu::PollType::Poll).is_err() {
            return;
        }
        self.inflight.retain(|staging| !staging.is_free());
    }

    /// Queue a copy from `staging` into `texture`; submitted by `flush`.
    pub(super) fn copy(&mut self, device: &wgpu::Device, staging: Arc<Staging>, texture: &wgpu::Texture) {
        let encoder = self.encoder.get_or_insert_with(|| device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("uploads") }));
        staging.record_copy(encoder, texture);
        self.batch.push(staging);
        self.recorded = true;
    }

    /// Unmap, submit once, then re-map so the next tick can write again.
    pub(super) fn flush(&mut self, queue: &wgpu::Queue) {
        if !self.recorded {
            return;
        }
        for staging in &self.batch {
            staging.buffer.unmap();
        }
        let Some(encoder) = self.encoder.take() else {
            return;
        };
        self.recorded = false;
        queue.submit(Some(encoder.finish()));
        for staging in self.batch.drain(..) {
            let free = staging.free.clone();
            free.store(false, Ordering::Release);
            staging.buffer.slice(..).map_async(wgpu::MapMode::Write, move |result| {
                free.store(result.is_ok(), Ordering::Release);
            });
            self.inflight.push(staging);
        }
    }
}

impl Compositor {
    /// Submit the copies queued by `ensure_texture` this tick.
    pub fn flush_uploads(&mut self, queue: &wgpu::Queue) {
        self.uploader.flush(queue);
    }
}
