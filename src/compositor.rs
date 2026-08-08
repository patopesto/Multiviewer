use crate::config::Grid;
use crate::source::{Frame, Registry};
use std::collections::HashMap;
use std::sync::Arc;

const MAX_CELLS: usize = 64; // 8x8 UI cap

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vert {
    pos: [f32; 2],
    uv: [f32; 2],
}

const SHADER: &str = r#"
struct VertIn { @location(0) pos: vec2<f32>, @location(1) uv: vec2<f32> };
struct VertOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs_main(in: VertIn) -> VertOut {
    var out: VertOut;
    out.pos = vec4<f32>(in.pos, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment fn fs_main(in: VertOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
"#;

/// Shared with the paint callback; immutable after creation.
pub struct Shared {
    pub pipeline: wgpu::RenderPipeline,
    pub placeholder_bg: Arc<wgpu::BindGroup>,
    pub vb: wgpu::Buffer,
    pub ib: wgpu::Buffer,
}

struct SourceTex {
    _tex: wgpu::Texture,
    bg: Arc<wgpu::BindGroup>,
    w: u32,
    h: u32,
    seq: u64,
}

pub struct Compositor {
    pub shared: Arc<Shared>,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<String, SourceTex>,
}

pub struct Draw {
    pub verts: Arc<Vec<Vert>>,
    /// (first_index, bind group) per grid cell, in cell order
    pub draws: Vec<(u32, Arc<wgpu::BindGroup>)>,
}

impl Compositor {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, target_format: wgpu::TextureFormat) -> Self {
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cell"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compositor"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compositor"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cells-vb"),
            size: (MAX_CELLS * 4 * std::mem::size_of::<Vert>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut indices = Vec::with_capacity(MAX_CELLS * 6);
        for i in 0..MAX_CELLS as u16 {
            let v = i * 4;
            indices.extend_from_slice(&[v, v + 1, v + 2, v + 2, v + 3, v]);
        }
        let ib = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cells-ib"),
            size: (indices.len() * 2) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ib, 0, bytemuck::cast_slice(&indices));

        let placeholder_bg = Arc::new(placeholder(device, &bind_layout, &sampler));

        Self {
            shared: Arc::new(Shared { pipeline, placeholder_bg, vb, ib }),
            bind_layout,
            sampler,
            textures: HashMap::new(),
        }
    }

    /// Per-frame: upload changed source textures, build quads for all cells.
    /// `rect` is the grid viewport in points; aspect fitting needs its shape.
    pub fn build(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        grid: &Grid,
        registry: &Registry,
        rect: egui::Rect,
    ) -> Draw {
        // one texture upload per unique source, even if shown in several cells
        let mut seen: HashMap<&str, Option<(Arc<wgpu::BindGroup>, f32)>> = HashMap::new();
        let mut verts = Vec::with_capacity(grid.cells.len() * 4);
        let mut draws = Vec::with_capacity(grid.cells.len());

        for (i, cell) in grid.cells.iter().enumerate() {
            let name = cell.as_deref();
            let entry = name.and_then(|n| {
                seen.entry(n)
                    .or_insert_with(|| {
                        let src = registry.get(n)?;
                        #[allow(irrefutable_let_patterns)] // phase 2 adds more Frame variants
                        let Frame::Cpu(f) = src.latest()?;
                        let st = self.ensure_texture(device, queue, n, &f);
                        Some((st.bg.clone(), f.w as f32 / f.h as f32))
                    })
                    .clone()
            });

            let (bg, aspect) = match entry {
                Some(e) => e,
                None => (self.shared.placeholder_bg.clone(), 16.0 / 9.0),
            };

            let col = (i as u32) % grid.cols;
            let row = (i as u32) / grid.cols;
            let x0 = col as f32 / grid.cols as f32 * 2.0 - 1.0;
            let x1 = (col + 1) as f32 / grid.cols as f32 * 2.0 - 1.0;
            let y0 = 1.0 - row as f32 / grid.rows as f32 * 2.0;
            let y1 = 1.0 - (row + 1) as f32 / grid.rows as f32 * 2.0;

            // fit source aspect into the cell, centered (letterbox)
            let cell_aspect =
                (rect.width() / grid.cols as f32) / (rect.height() / grid.rows as f32);
            let (mut sx, mut sy) = (1.0f32, 1.0f32);
            if aspect > cell_aspect {
                sy = cell_aspect / aspect;
            } else {
                sx = aspect / cell_aspect;
            }
            let cx = (x0 + x1) / 2.0;
            let cy = (y0 + y1) / 2.0;
            let hx = (x1 - x0) / 2.0 * sx;
            let hy = (y0 - y1) / 2.0 * sy;

            verts.extend_from_slice(&[
                Vert { pos: [cx - hx, cy + hy], uv: [0.0, 0.0] },
                Vert { pos: [cx + hx, cy + hy], uv: [1.0, 0.0] },
                Vert { pos: [cx + hx, cy - hy], uv: [1.0, 1.0] },
                Vert { pos: [cx - hx, cy - hy], uv: [0.0, 1.0] },
            ]);
            draws.push(((i * 6) as u32, bg));
        }

        Draw { verts: Arc::new(verts), draws }
    }

    fn ensure_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        f: &crate::source::CpuFrame,
    ) -> &SourceTex {
        let stale = self
            .textures
            .get(name)
            .map(|t| t.w != f.w || t.h != f.h)
            .unwrap_or(false);
        if stale {
            self.textures.remove(name);
        }
        let st = self.textures.entry(name.to_string()).or_insert_with(|| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(name),
                size: wgpu::Extent3d { width: f.w, height: f.h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            let bg = Arc::new(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(name),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            }));
            SourceTex { _tex: tex, bg, w: f.w, h: f.h, seq: u64::MAX }
        });
        if st.seq != f.seq {
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
                    bytes_per_row: Some(f.w * 4),
                    rows_per_image: Some(f.h),
                },
                wgpu::Extent3d { width: f.w, height: f.h, depth_or_array_layers: 1 },
            );
            st.seq = f.seq;
        }
        st
    }
}

fn placeholder(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("placeholder"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("placeholder"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
    // ponytail: texture never written → sample returns 0 (black). Good enough for empty cells.
}
