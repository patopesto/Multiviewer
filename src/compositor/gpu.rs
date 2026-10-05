use super::{ConvMode, ConvUniform, Vert};

pub(super) const MAX_LAYERS: usize = 256;

/// Vertex + fragment shader shared by every compositor pipeline.
pub(super) const SHADER: &str = r#"
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

/// Bind group layout shared by every compositor pipeline: source texture,
/// sampler and conversion uniform.
pub(super) fn create_cell_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
    })
}

pub(super) fn create_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    })
}

/// Render pipeline identical across compositor targets except label, format and blend.
pub(super) fn render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    format: wgpu::TextureFormat,
    blend: wgpu::BlendState,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vert>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Uniform block consumed by the fragment shader for one texture.
pub(super) fn conv_uniform(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mode: ConvMode,
    width: f32,
    height: f32,
) -> wgpu::Buffer {
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("conv"),
        size: std::mem::size_of::<ConvUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &uniform,
        0,
        bytemuck::cast_slice(&[ConvUniform {
            mode: mode as u32,
            width,
            height,
            _pad: 0.0,
        }]),
    );
    return uniform;
}

pub(super) fn texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    view: &wgpu::TextureView,
    uniform: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: uniform,
                    offset: 0,
                    size: None,
                }),
            },
        ],
    })
}

/// 1x1 RGBA texture; callers overwrite it for solid colors or leave it as passthrough.
pub(super) fn solid_texture(device: &wgpu::Device, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
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
    })
}

/// 1x1 passthrough bind group used before a source's first frame exists.
pub(super) fn placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let tex = solid_texture(device, "placeholder");
    let view = tex.create_view(&Default::default());
    let uniform = conv_uniform(device, queue, ConvMode::Passthrough, 1.0, 1.0);
    return texture_bind_group(device, layout, sampler, &view, &uniform);
}

pub(super) fn solid_bind_group(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    color: [u8; 4],
) -> wgpu::BindGroup {
    let tex = solid_texture(device, "solid-border");
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
    let uniform = conv_uniform(device, queue, ConvMode::Passthrough, 1.0, 1.0);
    return texture_bind_group(device, layout, sampler, &view, &uniform);
}

/// One content quad + up to four border edge quads + label bg + label text per source.
pub(super) const QUADS_PER_SOURCE: usize = 7;

/// Shared vertex + index buffers sized for `MAX_LAYERS` quads.
pub(super) fn create_vertex_index_buffers(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Buffer, wgpu::Buffer) {
    let vb = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cells-vb"),
        size: (MAX_LAYERS * QUADS_PER_SOURCE * 4 * std::mem::size_of::<Vert>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut indices = Vec::with_capacity(MAX_LAYERS * QUADS_PER_SOURCE * 6);
    for i in 0..(MAX_LAYERS * QUADS_PER_SOURCE) as u16 {
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
    return (vb, ib);
}
