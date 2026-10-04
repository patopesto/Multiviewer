use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use tracing::instrument;

use crate::config::{BorderVisibility, Canvas, LabelPosition, LabelVisibility, SourceBorderVisibility, SourceLabelVisibility, TextureMode};
use crate::sources::{CpuFrame, Frame, PixelFormat, SourceKey, SourceRegistry, SourceStats};

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

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConvMode {
    Passthrough = 0,
    UyvyBt601 = 1,
    UyvyBt709 = 2,
    Yuy2Bt601 = 3,
    Yuy2Bt709 = 4,
    Nv12Bt601 = 5,
    Nv12Bt709 = 6,
}

/// Uniform block consumed by the compositor's fragment shader.
/// Must stay in sync with the `ConvUniform` struct inside `SHADER`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ConvUniform {
    pub mode: u32, // ConvMode
    pub width: f32,
    pub height: f32,
    pub _pad: f32,
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

struct ConvUniform {
    mode: u32,
    width: f32,
    height: f32,
    _pad: f32,
};

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> conv: ConvUniform;

// YUV -> RGB (limited range, 8-bit). Shared by every 4:2:2/4:2:0 path;
fn yuv_to_rgb(yuv: vec3<f32>, bt709: bool) -> vec3<f32> {
    let y = yuv.x - 0.062745098; // 16/255
    let u = yuv.y - 0.5;
    let v = yuv.z - 0.5;

    var r: f32;
    var g: f32;
    var b: f32;
    if (bt709) {
        r = 1.164 * y + 1.793 * v;
        g = 1.164 * y - 0.213 * u - 0.533 * v;
        b = 1.164 * y + 2.112 * u;
    } else {
        r = 1.164 * y + 1.596 * v;
        g = 1.164 * y - 0.391 * u - 0.813 * v;
        b = 1.164 * y + 2.018 * u;
    }
    return clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Packed 4:2:2 stored as half-width Rgba8: one texel = two pixels.
//   UYVY: bytes (U, Y0, V, Y1) -> even x is Y0 (g), odd x is Y1 (a)
//   YUY2: bytes (Y0, U, Y1, V) -> even x is Y0 (r), odd x is Y1 (b)
fn fetch_422(x: i32, y: i32, yuy2: bool) -> vec3<f32> {
    let s = textureLoad(tex, vec2<i32>(x / 2, y), 0);
    let even = (x & 1) == 0;
    if (yuy2) {
        return vec3<f32>(select(s.b, s.r, even), s.g, s.a);
    }
    return vec3<f32>(select(s.a, s.g, even), s.r, s.b);
}

// NV12: one R8 texture, height 1.5h, Y plane then interleaved UV plane.
// The UV row for image row y lives at `plane + y/2`.
fn fetch_nv12_packed(x: i32, y: i32, plane: i32) -> vec3<f32> {
    let yy = textureLoad(tex, vec2<i32>(x, y), 0).r;
    let uvx = x & ~1;
    let uvrow = plane + y / 2;
    let uu = textureLoad(tex, vec2<i32>(uvx, uvrow), 0).r;
    let vv = textureLoad(tex, vec2<i32>(uvx + 1, uvrow), 0).r;
    return vec3<f32>(yy, uu, vv);
}

@fragment fn fs_main(in: VertOut) -> @location(0) vec4<f32> {
    if (conv.mode == 0u) {
        return textureSample(tex, samp, in.uv);
    }

    let x = clamp(i32(in.uv.x * conv.width), 0, i32(conv.width) - 1);
    let y = clamp(i32(in.uv.y * conv.height), 0, i32(conv.height) - 1);

    var yuv: vec3<f32>;
    var bt709: bool;
    if (conv.mode <= 4u) {
        yuv = fetch_422(x, y, conv.mode >= 3u);
        bt709 = (conv.mode == 2u) || (conv.mode == 4u);
    } else {
        let size = textureDimensions(tex);
        let plane = i32(size.y) * 2 / 3;
        yuv = fetch_nv12_packed(x, y, plane);
        bt709 = conv.mode == 6u;
    }
    return vec4<f32>(yuv_to_rgb(yuv, bt709), 1.0);
}
"#;

/// Shared with the paint callback; immutable after creation.
pub struct Shared {
    pub pipeline: wgpu::RenderPipeline,
    pub text_pipeline: wgpu::RenderPipeline,
    pub placeholder_bg: Arc<wgpu::BindGroup>,
    pub vb: wgpu::Buffer,
    pub ib: wgpu::Buffer,
}

struct SourceTex {
    _tex: wgpu::Texture,
    _uniform: wgpu::Buffer,
    bg: Arc<wgpu::BindGroup>,
    w: u32,
    h: u32,
    tex_w: u32,
    tex_h: u32,
    seq: u64,
    format: wgpu::TextureFormat,
    mode: ConvMode,
}

struct LabelTex {
    _tex: wgpu::Texture,
    bg: Arc<wgpu::BindGroup>,
    w: u32,
    h: u32,
    key_hash: u64,
}

#[derive(Clone, Debug)]
struct LabelKey {
    name: String,
    size: f32,
    text_color: [u8; 4],
}

impl Hash for LabelKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.size.to_bits().hash(state);
        self.text_color.hash(state);
    }
}

/// Per-source draw state resolved once per frame: (bind group, source aspect, flip_h, flip_v).
type ResolvedSource = Option<(Arc<wgpu::BindGroup>, f32, bool, bool)>;

pub struct Compositor {
    pub shared: Arc<Shared>,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<SourceKey, SourceTex>,
    pulls: HashMap<SourceKey, ResolvedSource>, // Resolved sources for the current frame
    canvas_texture: Option<wgpu::Texture>,
    canvas_view: Option<wgpu::TextureView>,
    canvas_vb: wgpu::Buffer,
    canvas_pipeline: wgpu::RenderPipeline,
    canvas_w: u32,
    canvas_h: u32,
    border_bg: Arc<wgpu::BindGroup>,
    border_color: [u8; 4],
    font: fontdue::Font,
    label_textures: HashMap<String, LabelTex>,
    label_bg_color: [u8; 4],
    label_bg_bg: Arc<wgpu::BindGroup>,
    text_pipeline: wgpu::RenderPipeline,
}

pub enum Pipeline {
    Main,
    Text,
}

pub struct DrawCall {
    pub first_index: u32,
    pub bind_group: Arc<wgpu::BindGroup>,
    pub pipeline: Pipeline,
}

pub struct Draw {
    pub verts: Arc<Vec<Vert>>,
    pub draws: Vec<DrawCall>,
}

impl Compositor {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> Self {
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
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

        // One content quad + up to four border edge quads + label bg + label text per source.
        const QUADS_PER_LAYER: usize = 7;
        let vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cells-vb"),
            size: (MAX_LAYERS * QUADS_PER_LAYER * 4 * std::mem::size_of::<Vert>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut indices = Vec::with_capacity(MAX_LAYERS * QUADS_PER_LAYER * 6);
        for i in 0..(MAX_LAYERS * QUADS_PER_LAYER) as u16 {
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

        let placeholder_bg = Arc::new(placeholder(device, queue, &bind_layout, &sampler));

        let border_color = [180, 180, 180, 255];
        let border_bg = Arc::new(solid_bind_group(
            device,
            queue,
            &bind_layout,
            &sampler,
            border_color,
        ));


        let canvas_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compositor-canvas"),
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
                    format: wgpu::TextureFormat::Bgra8Unorm,
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

        let canvas_vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas-vb"),
            size: (MAX_LAYERS * QUADS_PER_LAYER * 4 * std::mem::size_of::<Vert>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let font = fontdue::Font::from_bytes(
            epaint_default_fonts::HACK_REGULAR as &[u8],
            fontdue::FontSettings::default(),
        )
        .expect("embedded font is valid");

        let text_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compositor-text"),
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
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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

    /// Start a new frame: drop the resolved-source cache so the next build pulls each source again.
    pub fn begin_frame(&mut self) {
        self.pulls.clear();
    }

    /// Pull every source this frame will need before any render runs
    #[instrument(level = "debug", skip_all)]
    pub fn prefetch_sources(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        registry: &SourceRegistry,
        canvas: &Canvas,
        expanded_source: Option<&str>,
    ) {
        for source in &canvas.sources {
            if expanded_source.is_some() && expanded_source != Some(source.uuid.as_str()) {
                continue;
            }
            if let Some(source_ref) = source.source_ref.as_deref() {
                let key = SourceKey::new(source.protocol.clone(), source_ref.to_string());
                self.resolve_source(&key, registry, device, queue);
            }
        }
    }

    /// Pull a source once per frame.
    fn resolve_source(
        &mut self,
        key: &SourceKey,
        registry: &SourceRegistry,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> ResolvedSource {
        if let Some(cached) = self.pulls.get(key) {
            return cached.clone();
        }
        let resolved: ResolvedSource = (|| {
            let src = registry.get(key)?;
            let stats = src.stats();
            let latest_span = tracing::debug_span!("latest", source = %key.source_ref);
            let _latest_guard = latest_span.entered();
            let t = Instant::now();
            let frame = src.latest(device, queue)?;
            let receive_ms = t.elapsed().as_secs_f32() * 1000.0;
            {
                let mut s = stats.lock().unwrap();
                s.record_receive_time(receive_ms);
                s.record_consumed(frame.seq());
            }
            match frame {
                Frame::Cpu(f) => {
                    let st = self.ensure_texture(device, queue, key, &f, Some(stats));
                    Some((st.bg.clone(), f.w as f32 / f.h as f32, false, false))
                }
                Frame::Gpu(f) => {
                    Some((f.bg.clone(), f.w as f32 / f.h as f32, false, f.flip_v))
                }
            }
        })();
        self.pulls.insert(key.clone(), resolved.clone());
        return resolved;
    }

    /// Per-frame: upload changed source textures, build quads for all sources.
    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    pub fn build(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        canvas: &Canvas,
        registry: &SourceRegistry,
        panel_rect: &Rect,
        transform: (f32, f32, f32),
        expanded_source: Option<&str>,
    ) -> Draw {
        let (scale, offset_x, offset_y) = transform;
        let cx = panel_rect.x + offset_x;
        let cy = panel_rect.y + offset_y;
        let cw = canvas.width as f32 * scale;
        let ch = canvas.height as f32 * scale;

        if self.border_color != canvas.border.color {
            self.border_color = canvas.border.color;
            self.border_bg = Arc::new(solid_bind_group(
                device,
                queue,
                &self.bind_layout,
                &self.sampler,
                self.border_color,
            ));
        }

        let mut sources: Vec<_> = canvas.sources.iter().collect();
        sources.sort_by_key(|l| l.z);

        let mut verts = Vec::with_capacity(sources.len() * 7 * 4);
        let mut draws = Vec::with_capacity(sources.len() * 7);

        let mut first_index = 0u32;
        for source in sources {
            let is_expanded_source = expanded_source == Some(source.uuid.as_str());
            if expanded_source.is_some() && !is_expanded_source {
                continue;
            }

            // Per-frame dedupe: quads sharing a runtime source resolve once,
            // across builds of this frame and within this one.
            let entry = source.source_ref.as_deref().and_then(|source_ref| {
                let key = SourceKey::new(source.protocol.clone(), source_ref.to_string());
                self.resolve_source(&key, registry, device, queue)
            });

            let (bg, aspect, src_flip_h, src_flip_v) = match entry {
                Some(e) => e,
                None => (self.shared.placeholder_bg.clone(), 16.0 / 9.0, false, false),
            };
            let flip_h = src_flip_h ^ source.flip_h;
            let flip_v = src_flip_v ^ source.flip_v;

            let (lx, ly, lw, lh) = if is_expanded_source {
                (cx, cy, cw, ch)
            } else {
                (
                    cx + source.x * scale,
                    cy + source.y * scale,
                    source.width as f32 * scale,
                    source.height as f32 * scale,
                )
            };

            let x0 = (lx - panel_rect.x) / panel_rect.width() * 2.0 - 1.0;
            let x1 = (lx + lw - panel_rect.x) / panel_rect.width() * 2.0 - 1.0;
            let y0 = 1.0 - (ly - panel_rect.y) / panel_rect.height() * 2.0;
            let y1 = 1.0 - (ly + lh - panel_rect.y) / panel_rect.height() * 2.0;

            let layer_aspect = if lh > 0.0 { lw / lh } else { 1.0 };

            let (sx, sy, u0, u1, v0, v1) = match source.mode {
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

            let (u0, u1) = if flip_h { (u1, u0) } else { (u0, u1) };
            let (v0, v1) = if flip_v { (v1, v0) } else { (v0, v1) };

            let cx_ = (x0 + x1) / 2.0;
            let cy_ = (y0 + y1) / 2.0;
            let hx = (x1 - x0) / 2.0 * sx;
            let hy = (y0 - y1) / 2.0 * sy;

            verts.extend_from_slice(&[
                Vert {
                    pos: [cx_ - hx, cy_ + hy],
                    uv: [u0, v0],
                },
                Vert {
                    pos: [cx_ + hx, cy_ + hy],
                    uv: [u1, v0],
                },
                Vert {
                    pos: [cx_ + hx, cy_ - hy],
                    uv: [u1, v1],
                },
                Vert {
                    pos: [cx_ - hx, cy_ - hy],
                    uv: [u0, v1],
                },
            ]);
            draws.push(DrawCall {
                first_index,
                bind_group: bg,
                pipeline: Pipeline::Main,
            });
            first_index += 6;

            let global_borders = canvas.border.visibility;
            let border_visible = match source.border_visibility {
                SourceBorderVisibility::Show => true,
                SourceBorderVisibility::Hide => false,
                SourceBorderVisibility::Inherit => global_borders == BorderVisibility::Show,
            };
            if border_visible && !is_expanded_source {
                let border_px = canvas.border.width;
                let dx = 2.0 * border_px / panel_rect.width();
                let dy = 2.0 * border_px / panel_rect.height();

                // Top edge (inside source bounds).
                verts.extend_from_slice(&[
                    Vert {
                        pos: [x0, y0],
                        uv: [0.0, 0.0],
                    },
                    Vert {
                        pos: [x1, y0],
                        uv: [1.0, 0.0],
                    },
                    Vert {
                        pos: [x1, y0 - dy],
                        uv: [1.0, 1.0],
                    },
                    Vert {
                        pos: [x0, y0 - dy],
                        uv: [0.0, 1.0],
                    },
                ]);
                draws.push(DrawCall {
                    first_index,
                    bind_group: self.border_bg.clone(),
                    pipeline: Pipeline::Main,
                });
                first_index += 6;

                // Bottom edge (inside source bounds).
                verts.extend_from_slice(&[
                    Vert {
                        pos: [x0, y1 + dy],
                        uv: [0.0, 0.0],
                    },
                    Vert {
                        pos: [x1, y1 + dy],
                        uv: [1.0, 0.0],
                    },
                    Vert {
                        pos: [x1, y1],
                        uv: [1.0, 1.0],
                    },
                    Vert {
                        pos: [x0, y1],
                        uv: [0.0, 1.0],
                    },
                ]);
                draws.push(DrawCall {
                    first_index,
                    bind_group: self.border_bg.clone(),
                    pipeline: Pipeline::Main,
                });
                first_index += 6;

                // Left edge (inside source bounds).
                verts.extend_from_slice(&[
                    Vert {
                        pos: [x0, y0],
                        uv: [0.0, 0.0],
                    },
                    Vert {
                        pos: [x0 + dx, y0],
                        uv: [1.0, 0.0],
                    },
                    Vert {
                        pos: [x0 + dx, y1],
                        uv: [1.0, 1.0],
                    },
                    Vert {
                        pos: [x0, y1],
                        uv: [0.0, 1.0],
                    },
                ]);
                draws.push(DrawCall {
                    first_index,
                    bind_group: self.border_bg.clone(),
                    pipeline: Pipeline::Main,
                });
                first_index += 6;

                // Right edge (inside source bounds).
                verts.extend_from_slice(&[
                    Vert {
                        pos: [x1 - dx, y0],
                        uv: [0.0, 0.0],
                    },
                    Vert {
                        pos: [x1, y0],
                        uv: [1.0, 0.0],
                    },
                    Vert {
                        pos: [x1, y1],
                        uv: [1.0, 1.0],
                    },
                    Vert {
                        pos: [x1 - dx, y1],
                        uv: [0.0, 1.0],
                    },
                ]);
                draws.push(DrawCall {
                    first_index,
                    bind_group: self.border_bg.clone(),
                    pipeline: Pipeline::Main,
                });
                first_index += 6;
            }

            // Label overlay
            let label_visible = match source.label_visibility {
                SourceLabelVisibility::Show => true,
                SourceLabelVisibility::Hide => false,
                SourceLabelVisibility::Inherit => canvas.label.visibility == LabelVisibility::Show,
            };
            if label_visible && !source.name.is_empty() && !is_expanded_source {
                if self.label_bg_color != canvas.label.background_color {
                    self.label_bg_color = canvas.label.background_color;
                    self.label_bg_bg = Arc::new(solid_bind_group(
                        device,
                        queue,
                        &self.bind_layout,
                        &self.sampler,
                        self.label_bg_color,
                    ));
                }

                let label = &canvas.label;
                let label_key = LabelKey {
                    name: source.name.clone(),
                    size: label.size,
                    text_color: label.text_color,
                };
                let mut hasher = DefaultHasher::new();
                label_key.hash(&mut hasher);
                let key_hash = hasher.finish();

                let lt = self
                    .label_textures
                    .get(&source.uuid)
                    .filter(|t| t.key_hash == key_hash);

                let (tex_w, tex_h, label_bg) = if let Some(t) = lt {
                    (t.w, t.h, t.bg.clone())
                } else {
                    let (tex, bg, w, h) = self.rasterize_label(device, queue, &label_key);
                    self.label_textures.insert(
                        source.uuid.clone(),
                        LabelTex {
                            _tex: tex,
                            bg: bg.clone(),
                            w,
                            h,
                            key_hash,
                        },
                    );
                    (w, h, bg)
                };

                let padding = 4.0 * scale;
                let bg_w = (tex_w as f32 * scale + padding * 2.0).min(lw);
                let bg_h = (tex_h as f32 * scale + padding * 2.0).min(lh);

                let (bg_x, bg_y) = match label.position {
                    LabelPosition::TopLeft => (lx, ly),
                    LabelPosition::TopCenter => (lx + (lw - bg_w) / 2.0, ly),
                    LabelPosition::TopRight => (lx + lw - bg_w, ly),
                    LabelPosition::CenterLeft => (lx, ly + (lh - bg_h) / 2.0),
                    LabelPosition::Center => (lx + (lw - bg_w) / 2.0, ly + (lh - bg_h) / 2.0),
                    LabelPosition::CenterRight => (lx + lw - bg_w, ly + (lh - bg_h) / 2.0),
                    LabelPosition::BottomLeft => (lx, ly + lh - bg_h),
                    LabelPosition::BottomCenter => (lx + (lw - bg_w) / 2.0, ly + lh - bg_h),
                    LabelPosition::BottomRight => (lx + lw - bg_w, ly + lh - bg_h),
                };

                let bg_x0 = (bg_x - panel_rect.x) / panel_rect.width() * 2.0 - 1.0;
                let bg_x1 = (bg_x + bg_w - panel_rect.x) / panel_rect.width() * 2.0 - 1.0;
                let bg_y0 = 1.0 - (bg_y - panel_rect.y) / panel_rect.height() * 2.0;
                let bg_y1 = 1.0 - (bg_y + bg_h - panel_rect.y) / panel_rect.height() * 2.0;

                verts.extend_from_slice(&[
                    Vert {
                        pos: [bg_x0, bg_y0],
                        uv: [0.0, 0.0],
                    },
                    Vert {
                        pos: [bg_x1, bg_y0],
                        uv: [1.0, 0.0],
                    },
                    Vert {
                        pos: [bg_x1, bg_y1],
                        uv: [1.0, 1.0],
                    },
                    Vert {
                        pos: [bg_x0, bg_y1],
                        uv: [0.0, 1.0],
                    },
                ]);
                draws.push(DrawCall {
                    first_index,
                    bind_group: self.label_bg_bg.clone(),
                    pipeline: Pipeline::Text,
                });
                first_index += 6;

                let tx_w = tex_w as f32 * scale;
                let tx_h = tex_h as f32 * scale;
                let tx_x = bg_x + padding;
                let tx_y = bg_y + padding;

                let tx_x0 = (tx_x - panel_rect.x) / panel_rect.width() * 2.0 - 1.0;
                let tx_x1 = (tx_x + tx_w - panel_rect.x) / panel_rect.width() * 2.0 - 1.0;
                let tx_y0 = 1.0 - (tx_y - panel_rect.y) / panel_rect.height() * 2.0;
                let tx_y1 = 1.0 - (tx_y + tx_h - panel_rect.y) / panel_rect.height() * 2.0;

                verts.extend_from_slice(&[
                    Vert {
                        pos: [tx_x0, tx_y0],
                        uv: [0.0, 0.0],
                    },
                    Vert {
                        pos: [tx_x1, tx_y0],
                        uv: [1.0, 0.0],
                    },
                    Vert {
                        pos: [tx_x1, tx_y1],
                        uv: [1.0, 1.0],
                    },
                    Vert {
                        pos: [tx_x0, tx_y1],
                        uv: [0.0, 1.0],
                    },
                ]);
                draws.push(DrawCall {
                    first_index,
                    bind_group: label_bg,
                    pipeline: Pipeline::Text,
                });
                first_index += 6;
            }
        }

        let draw = Draw {
            verts: Arc::new(verts),
            draws,
        };
        return draw;
    }

    fn rasterize_label(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &LabelKey,
    ) -> (wgpu::Texture, Arc<wgpu::BindGroup>, u32, u32) {

        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.reset(&LayoutSettings::default());
        layout.append(&[&self.font], &TextStyle::new(&key.name, key.size, 0));

        let glyphs = layout.glyphs();
        if glyphs.is_empty() {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("label-empty"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("label-empty-conv"),
                size: std::mem::size_of::<ConvUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(
                &uniform,
                0,
                bytemuck::cast_slice(&[ConvUniform {
                    mode: ConvMode::Passthrough as u32,
                    width: 1.0,
                    height: 1.0,
                    _pad: 0.0,
                }]),
            );
            let bg = Arc::new(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("label-empty"),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &uniform,
                            offset: 0,
                            size: None,
                        }),
                    },
                ],
            }));
            return (tex, bg, 0, 0);
        }

        let max_x = glyphs.iter().map(|g| g.x + g.width as f32).fold(0.0_f32, f32::max).ceil() as i32;
        let min_y = glyphs.iter().map(|g| g.y.floor() as i32).min().unwrap_or(0);
        let max_y = glyphs.iter().map(|g| (g.y + g.height as f32).ceil() as i32).max().unwrap_or(0);
        let width = max_x.max(1) as u32;
        let height = (max_y - min_y).max(1) as u32;

        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let [tr, tg, tb, ta] = key.text_color;

        for glyph in glyphs {
            let (metrics, coverage) = self.font.rasterize_config(glyph.key);
            let gx = glyph.x as u32;
            let gy = (glyph.y as i32 - min_y) as u32;
            for row in 0..metrics.height as u32 {
                for col in 0..metrics.width as u32 {
                    let src_idx = (row * metrics.width as u32 + col) as usize;
                    let dst_x = gx + col;
                    let dst_y = gy + row;
                    if dst_x < width && dst_y < height {
                        let dst_idx = ((dst_y * width + dst_x) * 4) as usize;
                        let cov = coverage[src_idx];
                        // alpha-blend with existing pixel (simple over)
                        let src_a = ((cov as u32 * ta as u32) / 255) as u8;
                        let inv_dst_a = 255 - src_a;
                        pixels[dst_idx] = ((tr as u32 * src_a as u32 + pixels[dst_idx] as u32 * inv_dst_a as u32) / 255) as u8;
                        pixels[dst_idx + 1] = ((tg as u32 * src_a as u32 + pixels[dst_idx + 1] as u32 * inv_dst_a as u32) / 255) as u8;
                        pixels[dst_idx + 2] = ((tb as u32 * src_a as u32 + pixels[dst_idx + 2] as u32 * inv_dst_a as u32) / 255) as u8;
                        pixels[dst_idx + 3] = (src_a as u32 + (pixels[dst_idx + 3] as u32 * inv_dst_a as u32) / 255) as u8;
                    }
                }
            }
        }

        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("label"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("label-conv"),
            size: std::mem::size_of::<ConvUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &uniform,
            0,
            bytemuck::cast_slice(&[ConvUniform {
                mode: ConvMode::Passthrough as u32,
                width: width as f32,
                height: height as f32,
                _pad: 0.0,
            }]),
        );
        let bg = Arc::new(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("label"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniform,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        }));

        (tex, bg, width, height)
    }

    #[instrument(level = "debug", skip_all)]
    pub fn render_canvas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        canvas: &Canvas,
        registry: &SourceRegistry,
        expanded_source: Option<&str>,
    ) {
        let canvas_w = canvas.width;
        let canvas_h = canvas.height;

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

        let panel_rect = Rect {
            x: 0.0,
            y: 0.0,
            w: canvas_w as f32,
            h: canvas_h as f32,
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

            rpass.set_viewport(0.0, 0.0, canvas_w as f32, canvas_h as f32, 0.0, 1.0);
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

    pub fn canvas_texture(&self) -> Option<(&wgpu::Texture, u32, u32)> {
        self.canvas_texture
            .as_ref()
            .map(|t| (t, self.canvas_w, self.canvas_h))
    }

    fn ensure_texture(
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
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("{}-conv", key.source_ref)),
                size: std::mem::size_of::<ConvUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(
                &uniform,
                0,
                bytemuck::cast_slice(&[ConvUniform {
                    mode: mode as u32,
                    width: f.w as f32,
                    height: f.h as f32,
                    _pad: 0.0,
                }]),
            );
            let bg = Arc::new(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(key.source_ref.as_str()),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &uniform,
                            offset: 0,
                            size: None,
                        }),
                    },
                ],
            }));
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
        st
    }
}

/// Texture layout for a CPU frame
fn source_layout(
    fmt: PixelFormat,
    w: u32,
    h: u32,
) -> (wgpu::TextureFormat, u32, u32, u32, ConvMode) {
    let hd = h > 576;
    return match fmt {
        PixelFormat::Rgba8 => (wgpu::TextureFormat::Rgba8Unorm, w, h, 4, ConvMode::Passthrough),
        PixelFormat::Bgra8 => (wgpu::TextureFormat::Bgra8Unorm, w, h, 4, ConvMode::Passthrough),
        // 4:2:2 packed as Rgba8 at half width (two pixels per texel).
        PixelFormat::Uyvy422 => {
            let mode = if hd { ConvMode::UyvyBt709 } else { ConvMode::UyvyBt601 };
            (wgpu::TextureFormat::Rgba8Unorm, w / 2, h, 2, mode)
        }
        PixelFormat::Yuy2 => {
            let mode = if hd { ConvMode::Yuy2Bt709 } else { ConvMode::Yuy2Bt601 };
            (wgpu::TextureFormat::Rgba8Unorm, w / 2, h, 2, mode)
        }
        // 4:2:0: one R8 texture, Y plane then interleaved UV plane (height 1.5h).
        PixelFormat::Nv12 => {
            let mode = if hd { ConvMode::Nv12Bt709 } else { ConvMode::Nv12Bt601 };
            (wgpu::TextureFormat::R8Unorm, w, h + h / 2, 1, mode)
        }
    };
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
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("placeholder"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("placeholder-conv"),
        size: std::mem::size_of::<ConvUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &uniform,
        0,
        bytemuck::cast_slice(&[ConvUniform {
            mode: ConvMode::Passthrough as u32,
            width: 1.0,
            height: 1.0,
            _pad: 0.0,
        }]),
    );
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("placeholder"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform,
                    offset: 0,
                    size: None,
                }),
            },
        ],
    })
}

fn solid_bind_group(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    color: [u8; 4],
) -> wgpu::BindGroup {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("solid-border"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &color,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("solid-border-conv"),
        size: std::mem::size_of::<ConvUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &uniform,
        0,
        bytemuck::cast_slice(&[ConvUniform {
            mode: ConvMode::Passthrough as u32,
            width: 1.0,
            height: 1.0,
            _pad: 0.0,
        }]),
    );
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("solid-border"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform,
                    offset: 0,
                    size: None,
                }),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    /// CPU reference matching the old DeckLink shim fixed-point conversion.
    fn ref_uyvy_to_rgb(u: u8, y: u8, v: u8, hd: bool) -> [u8; 3] {
        let (rc, gc_u, gc_v, bc) = if hd {
            (459, 55, 136, 541)
        } else {
            (409, 100, 208, 516)
        };
        let u = u as i32 - 128;
        let y = y as i32 - 16;
        let v = v as i32 - 128;
        let r = (298 * y + rc * v + 128) >> 8;
        let g = (298 * y - gc_u * u - gc_v * v + 128) >> 8;
        let b = (298 * y + bc * u + 128) >> 8;
        let clamp = |v: i32| v.clamp(0, 255) as u8;
        [clamp(r), clamp(g), clamp(b)]
    }

    /// GPU shader equivalent (floating-point) for UYVY -> RGB.
    fn shader_uyvy_to_rgb(u: u8, y: u8, v: u8, hd: bool) -> [u8; 3] {
        let uf = (u as f32 - 128.0) / 255.0;
        let yf = (y as f32 - 16.0) / 255.0;
        let vf = (v as f32 - 128.0) / 255.0;
        let (r, g, b) = if hd {
            (
                1.164 * yf + 1.793 * vf,
                1.164 * yf - 0.213 * uf - 0.533 * vf,
                1.164 * yf + 2.112 * uf,
            )
        } else {
            (
                1.164 * yf + 1.596 * vf,
                1.164 * yf - 0.391 * uf - 0.813 * vf,
                1.164 * yf + 2.018 * uf,
            )
        };
        let clamp = |v: f32| (v * 255.0).clamp(0.0, 255.0).round() as u8;
        [clamp(r), clamp(g), clamp(b)]
    }

    #[test]
    fn uyvy_to_rgb_matches_cpu_reference() {
        let test_values: &[(u8, u8, u8)] = &[
            (128, 235, 128), // white
            (128, 16, 128),  // black
            (240, 180, 128), // yellow-ish
            (128, 168, 184), // cyan-ish
            (0, 81, 240),    // red-ish
            (0, 145, 54),    // green-ish
        ];
        for &(u, y, v) in test_values {
            for &hd in &[false, true] {
                let expected = ref_uyvy_to_rgb(u, y, v, hd);
                let actual = shader_uyvy_to_rgb(u, y, v, hd);
                assert!(
                    expected
                        .iter()
                        .zip(&actual)
                        .all(|(e, a)| e.abs_diff(*a) <= 1),
                    "mismatch for U={u} Y={y} V={v} hd={hd}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }

    #[test]
    fn source_layout_matches_each_format() {
        use super::{source_layout, ConvMode};
        use crate::sources::PixelFormat;
        use wgpu::TextureFormat;

        assert_eq!(
            source_layout(PixelFormat::Bgra8, 1280, 720),
            (TextureFormat::Bgra8Unorm, 1280, 720, 4, ConvMode::Passthrough)
        );
        // 4:2:2 packs two pixels per texel: half width, so Rgba8 rows are w*2 bytes.
        assert_eq!(
            source_layout(PixelFormat::Uyvy422, 1280, 720),
            (TextureFormat::Rgba8Unorm, 640, 720, 2, ConvMode::UyvyBt709)
        );
        assert_eq!(
            source_layout(PixelFormat::Yuy2, 640, 480),
            (TextureFormat::Rgba8Unorm, 320, 480, 2, ConvMode::Yuy2Bt601)
        );
        // NV12 keeps full width; the texture is 1.5x tall (Y + interleaved UV).
        assert_eq!(
            source_layout(PixelFormat::Nv12, 1920, 1080),
            (TextureFormat::R8Unorm, 1920, 1620, 1, ConvMode::Nv12Bt709)
        );
    }

    /// The WGSL is normally only parsed/validated when the app starts; this
    /// catches syntax/semantic errors without a GPU device.
    #[test]
    fn shader_parses_and_validates() {
        let module =
            wgpu::naga::front::wgsl::parse_str(super::SHADER).expect("WGSL failed to parse");
        let mut validator = wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("WGSL failed validation");
    }

    /// UYVY and YUY2 produce the same texture format/dimensions, so `mode` must
    /// be part of the texture staleness key — otherwise switching between them
    /// reuses the wrong shader decode (YUY2 bytes decoded as UYVY gives the
    /// green/pink artefact).
    #[test]
    fn uyvy_and_yuy2_share_layout_but_differ_in_mode() {
        use super::source_layout;
        use crate::sources::PixelFormat;

        let (f1, w1, h1, _, m1) = source_layout(PixelFormat::Uyvy422, 1280, 720);
        let (f2, w2, h2, _, m2) = source_layout(PixelFormat::Yuy2, 1280, 720);
        assert_eq!((f1, w1, h1), (f2, w2, h2));
        assert_ne!(m1, m2);
    }
}
