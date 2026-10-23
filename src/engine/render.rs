use std::sync::Arc;

use super::Engine;
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
