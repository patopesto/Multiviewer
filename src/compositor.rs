use crate::config::{Canvas, TextureMode};
use crate::sources::{Frame, Registry};
use std::collections::HashMap;
use std::sync::Arc;

const MAX_LAYERS: usize = 256;

/// A simple rectangle, replacing the previous egui::Rect dependency.
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.w
    }
    pub fn height(&self) -> f32 {
        self.h
    }
}

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
    format: wgpu::TextureFormat,
}

pub struct Compositor {
    pub shared: Arc<Shared>,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<String, SourceTex>,
}

pub struct Draw {
    pub verts: Arc<Vec<Vert>>,
    /// (first_index, bind group) per layer, in z-order
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
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
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
            multiview_mask: None,
            cache: None,
        });

        let vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cells-vb"),
            size: (MAX_LAYERS * 4 * std::mem::size_of::<Vert>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut indices = Vec::with_capacity(MAX_LAYERS * 6);
        for i in 0..MAX_LAYERS as u16 {
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

    /// Per-frame: upload changed source textures, build quads for all layers.
    pub fn build(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        canvas: &Canvas,
        registry: &Registry,
        panel_rect: &Rect,
    ) -> Draw {
        let (scale, offset_x, offset_y) = canvas_transform(canvas, panel_rect);
        let cx = panel_rect.x + offset_x;
        let cy = panel_rect.y + offset_y;

        let mut seen: HashMap<&str, Option<(Arc<wgpu::BindGroup>, f32, bool)>> = HashMap::new();

        let mut layers: Vec<_> = canvas.layers.iter().collect();
        layers.sort_by_key(|l| l.z);

        let mut verts = Vec::with_capacity(layers.len() * 4);
        let mut draws = Vec::with_capacity(layers.len());

        for (i, layer) in layers.iter().enumerate() {
            let entry = layer.source_id.as_deref().and_then(|sid| {
                seen.entry(sid)
                    .or_insert_with(|| {
                        let src = registry.get(&sid.to_string())?;
                        match src.latest(device, queue)? {
                            Frame::Cpu(f) => {
                                let st = self.ensure_texture(device, queue, sid, &f);
                                Some((st.bg.clone(), f.w as f32 / f.h as f32, false))
                            }
                            Frame::Syphon(f) => {
                                Some((f.bg.clone(), f.w as f32 / f.h as f32, true))
                            }
                        }
                    })
                    .clone()
            });

            let (bg, aspect, is_syphon) = match entry {
                Some(e) => e,
                None => (self.shared.placeholder_bg.clone(), 16.0 / 9.0, false),
            };

            let lx = cx + layer.x * scale;
            let ly = cy + layer.y * scale;
            let lw = layer.width as f32 * scale;
            let lh = layer.height as f32 * scale;

            let x0 = (lx - panel_rect.x) / panel_rect.width() * 2.0 - 1.0;
            let x1 = (lx + lw - panel_rect.x) / panel_rect.width() * 2.0 - 1.0;
            let y0 = 1.0 - (ly - panel_rect.y) / panel_rect.height() * 2.0;
            let y1 = 1.0 - (ly + lh - panel_rect.y) / panel_rect.height() * 2.0;

            let layer_aspect = if lh > 0.0 { lw / lh } else { 1.0 };

            let (sx, sy, u0, u1, v0, v1) = match layer.mode {
                TextureMode::Fit => {
                    if aspect > layer_aspect {
                        let sy = layer_aspect / aspect;
                        (1.0f32, sy, 0.0, 1.0, 0.0, 1.0)
                    } else {
                        let sx = aspect / layer_aspect;
                        (sx, 1.0, 0.0, 1.0, 0.0, 1.0)
                    }
                }
                TextureMode::Fill => {
                    if aspect > layer_aspect {
                        let u_scale = layer_aspect / aspect;
                        let uc = 0.5;
                        (1.0, 1.0, uc - u_scale / 2.0, uc + u_scale / 2.0, 0.0, 1.0)
                    } else {
                        let v_scale = aspect / layer_aspect;
                        let vc = 0.5;
                        (1.0, 1.0, 0.0, 1.0, vc - v_scale / 2.0, vc + v_scale / 2.0)
                    }
                }
                TextureMode::Stretch => (1.0, 1.0, 0.0, 1.0, 0.0, 1.0),
            };

            let (v0, v1) = if is_syphon { (v1, v0) } else { (v0, v1) };

            let cx_ = (x0 + x1) / 2.0;
            let cy_ = (y0 + y1) / 2.0;
            let hx = (x1 - x0) / 2.0 * sx;
            let hy = (y0 - y1) / 2.0 * sy;

            verts.extend_from_slice(&[
                Vert { pos: [cx_ - hx, cy_ + hy], uv: [u0, v0] },
                Vert { pos: [cx_ + hx, cy_ + hy], uv: [u1, v0] },
                Vert { pos: [cx_ + hx, cy_ - hy], uv: [u1, v1] },
                Vert { pos: [cx_ - hx, cy_ - hy], uv: [u0, v1] },
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
        f: &crate::sources::CpuFrame,
    ) -> &SourceTex {
        use crate::sources::PixelFormat;
        let format = match f.fmt {
            PixelFormat::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
            PixelFormat::Bgra8 => wgpu::TextureFormat::Bgra8Unorm,
        };
        let stale = self
            .textures
            .get(name)
            .map(|t| t.w != f.w || t.h != f.h || t.format != format)
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
                format,
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
            SourceTex { _tex: tex, bg, w: f.w, h: f.h, seq: u64::MAX, format }
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

/// Compute the scale and offset to letterbox the canvas inside the panel.
pub fn canvas_transform(canvas: &Canvas, panel_rect: &Rect) -> (f32, f32, f32) {
    let canvas_aspect = canvas.width as f32 / canvas.height.max(1) as f32;
    let panel_aspect = panel_rect.width() / panel_rect.height().max(0.001);
    if canvas_aspect > panel_aspect {
        let scale = panel_rect.width() / canvas.width.max(1) as f32;
        let h = canvas.height as f32 * scale;
        (scale, 0.0, (panel_rect.height() - h) / 2.0)
    } else {
        let scale = panel_rect.height() / canvas.height.max(1) as f32;
        let w = canvas.width as f32 * scale;
        (scale, (panel_rect.width() - w) / 2.0, 0.0)
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
        format: wgpu::TextureFormat::Rgba8Unorm,
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
}
