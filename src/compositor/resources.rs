use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::gpu::{conv_uniform, create_cell_layout, create_sampler, create_vertex_index_buffers, placeholder, render_pipeline, solid_bind_group, texture_bind_group, MAX_LAYERS, QUADS_PER_SOURCE, SHADER};
use super::layout::source_layout;
use super::{Compositor, ConvMode, Shared, Vert};
use crate::sources::{CpuFrame, PixelFormat, SourceKey, SourceStats};

pub(super) struct SourceTex {
    _tex: wgpu::Texture,
    _uniform: wgpu::Buffer,
    pub(super) bg: Arc<wgpu::BindGroup>,
    w: u32,
    h: u32,
    tex_w: u32,
    tex_h: u32,
    seq: u64,
    format: wgpu::TextureFormat,
    mode: ConvMode,
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
            canvas_texture: None,
            canvas_view: None,
            canvas_vb,
            canvas_pipeline,
            canvas_w: 0,
            canvas_h: 0,
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
        let (format, tex_w, tex_h, bpp, mode) = source_layout(f.fmt, f.w, f.h);
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
            .map(|t| {
                t.w != f.w
                    || t.h != f.h
                    || t.format != format
                    || t.tex_w != tex_w
                    || t.tex_h != tex_h
                    || t.mode != mode
            })
            .unwrap_or(false);
        if stale {
            self.textures.remove(key);
        }
        let st = self.textures.entry(key.clone()).or_insert_with(|| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(key.source_ref.as_str()),
                size: wgpu::Extent3d {
                    width: tex_w,
                    height: tex_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            let uniform = conv_uniform(device, queue, mode, f.w as f32, f.h as f32);
            let bg = Arc::new(texture_bind_group(
                device,
                &self.bind_layout,
                &self.sampler,
                &view,
                &uniform,
            ));
            SourceTex {
                _tex: tex,
                _uniform: uniform,
                bg,
                w: f.w,
                h: f.h,
                tex_w,
                tex_h,
                seq: u64::MAX,
                format,
                mode,
            }
        });
        if st.seq != f.seq {
            let upload_span = tracing::debug_span!("upload", source = %key.source_ref);
            let _upload_guard = upload_span.entered();
            let t0 = Instant::now();
            // Padded frames carry their native stride; 0 = tightly packed.
            let pitch = if f.pitch != 0 { f.pitch } else { f.w * bpp };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &st._tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &f.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: Some(tex_h),
                },
                wgpu::Extent3d {
                    width: tex_w,
                    height: tex_h,
                    depth_or_array_layers: 1,
                },
            );
            let upload_ms = t0.elapsed().as_secs_f32() * 1000.0;
            if let Some(stats) = stats {
                let mut s = stats.lock().unwrap();
                s.record_upload_time(upload_ms, pitch as u64 * tex_h as u64);
            }
            st.seq = f.seq;
        }
        return st;
    }
}
