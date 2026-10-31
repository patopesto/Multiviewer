use std::sync::Arc;

use super::{Engine, RunTotals};
use crate::compositor::{Compositor, Draw, Rect, Shared};

impl Engine {
    pub fn ensure_compositor(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) {
        if self.comp.is_none() {
            self.comp = Some(Compositor::new(device, queue, target_format));
        }
    }

    /// Close out one tick: emit its counters and fold them into the run totals.
    /// Returns the smoothed GPU pass time for the status bar.
    pub fn finish_tick(&mut self, tick_ms: f32) -> Option<f32> {
        let gpu_ms = self.comp.as_mut().and_then(Compositor::gpu_ms);
        let stats = self.comp.as_ref().map(Compositor::frame_stats).unwrap_or_default();
        stats.report(self.cfg.canvas.sources.len(), tick_ms, gpu_ms);
        self.totals.ticks += 1;
        self.totals.work_ms += tick_ms as f64;
        if let Some(gpu) = gpu_ms {
            self.totals.gpu_ms += gpu as f64;
            self.totals.gpu_samples += 1;
        }
        self.totals.uploads += stats.uploads as u64;
        self.totals.upload_bytes += stats.upload_bytes;
        self.totals.upload_fallbacks += stats.upload_fallbacks as u64;
        return gpu_ms;
    }

    pub fn run_totals(&self) -> RunTotals {
        return self.totals;
    }

    /// Start a new frame: clear the compositor's per-frame source cache and pull every canvas source once
    pub fn begin_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(comp) = self.comp.as_mut() else {
            return;
        };
        comp.begin_frame();
        comp.prefetch_sources(
            device,
            queue,
            &self.registry,
            &self.cfg.canvas,
            self.expanded_source_id.as_deref(),
        );
        comp.flush_uploads(queue);
    }

    pub fn build_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        panel_rect: &Rect,
        transform: (f32, f32, f32),
    ) -> Draw {
        let comp = self.comp.as_mut().expect("compositor not initialized");
        comp.build(
            device,
            queue,
            &self.cfg.canvas,
            &self.registry,
            panel_rect,
            transform,
            self.expanded_source_id.as_deref(),
        )
    }

    pub fn render_outputs(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if !self.output_registry.any_ready() {
            return;
        }
        let comp = self.comp.as_mut().expect("compositor not initialized");
        comp.render_canvas(
            device,
            queue,
            &self.cfg.canvas,
            &self.registry,
            self.expanded_source_id.as_deref(),
        );
        if let Some((texture, w, h)) = comp.canvas_texture() {
            self.output_registry.present_all(texture, w, h, device, queue);
        }
    }

    pub fn shared(&self) -> Option<Arc<Shared>> {
        self.comp.as_ref().map(|c| c.shared.clone())
    }
}
