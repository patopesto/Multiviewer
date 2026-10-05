use tracing::instrument;

use super::{Compositor, Pipeline, Rect};
use crate::config::Canvas;
use crate::sources::SourceRegistry;

impl Compositor {
    #[instrument(level = "debug", skip_all)]
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
        let draw = self.build(
            device,
            queue,
            canvas,
            registry,
            &panel_rect,
            transform,
            expanded_source,
        );

        queue.write_buffer(&self.canvas_vb, 0, bytemuck::cast_slice(&draw.verts));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("canvas-output"),
        });

        {
            let view = self.canvas_view.as_ref().unwrap();
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
                timestamp_writes: None,
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

        queue.submit(Some(encoder.finish()));
    }

    fn ensure_canvas_texture(&mut self, device: &wgpu::Device, canvas_w: u32, canvas_h: u32) {
        if self.canvas_texture.is_none()
            || self.canvas_w != canvas_w
            || self.canvas_h != canvas_h
        {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("canvas-output"),
                size: wgpu::Extent3d {
                    width: canvas_w,
                    height: canvas_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.canvas_texture = Some(texture);
            self.canvas_view = Some(view);
            self.canvas_w = canvas_w;
            self.canvas_h = canvas_h;
        }
    }

    pub fn canvas_texture(&self) -> Option<(&wgpu::Texture, u32, u32)> {
        self.canvas_texture
            .as_ref()
            .map(|t| (t, self.canvas_w, self.canvas_h))
    }
}
