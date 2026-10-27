use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::gpu::{conv_uniform, create_cell_layout, create_sampler, create_vertex_index_buffers, placeholder, render_pipeline, solid_bind_group, texture_bind_group, MAX_LAYERS, QUADS_PER_SOURCE, SHADER};
use super::{Compositor, Shared, Vert};
use crate::sources::{
    CpuFrame, PixelFormat, SourceKey, SourceStats, TextureTarget, UPLOAD_TARGET_USAGE,
};

pub(super) struct SourceTex {
    target: TextureTarget,
    _uniform: wgpu::Buffer,
    pub(super) bg: Arc<wgpu::BindGroup>,
    seq: u64,
}

impl Compositor {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let bind_layout = create_cell_layout(device);
        let sampler = create_sampler(device);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compositor"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = render_pipeline(
            device,
            &layout,
            &shader,
            "compositor",
            target_format,
            wgpu::BlendState::REPLACE,
        );

        let (vb, ib) = create_vertex_index_buffers(device, queue);
        let placeholder_bg = Arc::new(placeholder(device, queue, &bind_layout, &sampler));

        let border_color = [180, 180, 180, 255];
        let border_bg = Arc::new(solid_bind_group(
            device,
            queue,
            &bind_layout,
            &sampler,
            border_color,
        ));

        let canvas_pipeline = render_pipeline(
            device,
            &layout,
            &shader,
            "compositor-canvas",
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::BlendState::REPLACE,
        );
        let canvas_vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas-vb"),
            size: (MAX_LAYERS * QUADS_PER_SOURCE * 4 * std::mem::size_of::<Vert>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let font = fontdue::Font::from_bytes(
            epaint_default_fonts::HACK_REGULAR as &[u8],
            fontdue::FontSettings::default(),
        )
        .expect("embedded font is valid");

        let text_pipeline = render_pipeline(
            device,
            &layout,
            &shader,
            "compositor-text",
            target_format,
            wgpu::BlendState::ALPHA_BLENDING,
        );

        let label_bg_color = [0, 0, 0, 180];
        let label_bg_bg = Arc::new(solid_bind_group(
            device,
            queue,
            &bind_layout,
            &sampler,
            label_bg_color,
        ));

        Self {
            shared: Arc::new(Shared {
                pipeline,
                text_pipeline: text_pipeline.clone(),
                placeholder_bg,
                vb,
                ib,
            }),
            bind_layout,
            sampler,
            textures: HashMap::new(),
            pulls: HashMap::new(),
            canvas: None,
            canvas_vb,
            canvas_pipeline,
            border_bg,
            border_color,
            font,
            label_textures: HashMap::new(),
            label_bg_color,
            label_bg_bg,
            text_pipeline,
        }
    }

    pub(super) fn ensure_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &SourceKey,
        f: &CpuFrame,
        stats: Option<Arc<Mutex<SourceStats>>>,
    ) -> &SourceTex {
        let layout = f.fmt.texture_layout(f.w, f.h);
        if matches!(f.fmt, PixelFormat::Uyvy422 | PixelFormat::Yuy2 | PixelFormat::Nv12)
            && !f.w.is_multiple_of(2)
        {
            tracing::warn!("compositor {}: {} frame has odd width {}, last column will be dropped", key.source_ref, f.fmt.label(), f.w);
        }
        if f.fmt == PixelFormat::Nv12 && !f.h.is_multiple_of(2) {
            tracing::warn!("compositor {}: NV12 frame has odd height {}, last row will be dropped", key.source_ref, f.h);
        }

        let stale = self
            .textures
            .get(key)
            .map(|t| t.target.layout != layout)
            .unwrap_or(false);
        if stale {
            self.textures.remove(key);
        }
        let st = self.textures.entry(key.clone()).or_insert_with(|| {
            let mut target = None;
            TextureTarget::ensure(
                &mut target,
                device,
                layout,
                UPLOAD_TARGET_USAGE,
                key.source_ref.as_str(),
            );
            let target = target.unwrap();
            let uniform = conv_uniform(device, queue, layout.conv_mode, f.w as f32, f.h as f32);
            let bg = Arc::new(texture_bind_group(
                device,
                &self.bind_layout,
                &self.sampler,
                &target.view,
                &uniform,
            ));
            SourceTex {
                target,
                _uniform: uniform,
                bg,
                seq: u64::MAX,
            }
        });
        if st.seq != f.seq {
            let upload_span = tracing::debug_span!("upload", source = %key.source_ref);
            let _upload_guard = upload_span.entered();
            let t0 = Instant::now();
            // Padded frames carry their native stride; 0 = tightly packed.
            let pitch = if f.pitch != 0 { f.pitch } else { f.w * layout.bpp };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &st.target.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &f.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: Some(layout.tex_h),
                },
                wgpu::Extent3d {
                    width: layout.tex_w,
                    height: layout.tex_h,
                    depth_or_array_layers: 1,
                },
            );
            let upload_ms = t0.elapsed().as_secs_f32() * 1000.0;
            if let Some(stats) = stats {
                let mut s = stats.lock().unwrap();
                s.record_upload_time(upload_ms, pitch as u64 * layout.tex_h as u64);
            }
            st.seq = f.seq;
        }
        return st;
    }
}
