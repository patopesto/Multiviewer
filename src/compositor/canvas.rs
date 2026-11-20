use tracing::instrument;

use super::metrics::GpuTimers;
use super::{Compositor, Pipeline, Rect};
use crate::config::Canvas;
use crate::sources::{PixelFormat, RENDER_TARGET_USAGE, SourceRegistry, TextureTarget};

impl Compositor {
    #[instrument(
        level = "debug",
        skip_all,
        fields(width = canvas.width, height = canvas.height, draws = tracing::field::Empty)
    )]
    pub fn render_canvas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        canvas: &Canvas,
        registry: &SourceRegistry,
        expanded_source: Option<&str>,
    ) {
        self.ensure_canvas_texture(device, canvas.width, canvas.height);

        let panel_rect = Rect {
            x: 0.0,
            y: 0.0,
            w: canvas.width as f32,
            h: canvas.height as f32,
        };
        let transform = (1.0, 0.0, 0.0);
        let draw = self.build(device, queue, canvas, registry, &panel_rect, transform, expanded_source, 1.0);

        queue.write_buffer(&self.canvas_vb, 0, bytemuck::cast_slice(&draw.verts));

        let timer_slot = self.timers.as_ref().and_then(GpuTimers::free_slot);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("canvas-output"),
        });

        {
            let view = &self.canvas.as_ref().unwrap().view;
            let timestamp_writes = match (self.timers.as_ref(), timer_slot) {
                (Some(timers), Some(slot)) => Some(timers.writes(slot)),
                _ => None,
            };
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("canvas-output"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            rpass.set_viewport(0.0, 0.0, canvas.width as f32, canvas.height as f32, 0.0, 1.0);
            rpass.set_vertex_buffer(0, self.canvas_vb.slice(..));
            rpass.set_index_buffer(self.shared.ib.slice(..), wgpu::IndexFormat::Uint16);
            let mut current_pipeline = None;
            for draw_call in &draw.draws {
                let pipeline = match draw_call.pipeline {
                    Pipeline::Main => &self.canvas_pipeline,
                    Pipeline::Text => &self.text_pipeline,
                };
                if current_pipeline != Some(pipeline) {
                    rpass.set_pipeline(pipeline);
                    current_pipeline = Some(pipeline);
                }
                rpass.set_bind_group(0, &*draw_call.bind_group, &[]);
                rpass.draw_indexed(draw_call.first_index..draw_call.first_index + 6, 0, 0..1);
            }
        }

        if let (Some(timers), Some(slot)) = (self.timers.as_ref(), timer_slot) {
            timers.encode_readback(&mut encoder, slot);
        }
        queue.submit(Some(encoder.finish()));
        if let (Some(timers), Some(slot)) = (self.timers.as_mut(), timer_slot) {
            timers.arm(slot);
        }

        self.frame_stats.canvas_pass(draw.draws.len() as u32, draw.verts.len() as u32);
        tracing::Span::current().record("draws", draw.draws.len());
    }

    fn ensure_canvas_texture(&mut self, device: &wgpu::Device, canvas_w: u32, canvas_h: u32) {
        TextureTarget::ensure(
            &mut self.canvas,
            device,
            PixelFormat::Bgra8.texture_layout(canvas_w, canvas_h),
            RENDER_TARGET_USAGE,
            "canvas-output",
        );
    }

    pub fn canvas_texture(&self) -> Option<(&wgpu::Texture, u32, u32)> {
        return self.canvas.as_ref().map(|target| (&target.texture, target.layout.width, target.layout.height));
    }
}
