use super::common::{ColorRange, ColorSpace, ConvUniform, TextureLayout, TextureTarget};

const SHADER: &str = r#"
struct VertIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct ConvUniform {
    mode: u32,
    width: f32,
    height: f32,
    _pad: f32,
};

@vertex
fn vs_main(in: VertIn) -> VertOut {
    var out: VertOut;
    out.position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> conv: ConvUniform;

// Full-range YUV coefficients. The range conversion happens in encode().
fn rgb_to_yuv(rgb: vec3<f32>, standard: u32) -> vec3<f32> {
    var y: f32;
    var u: f32;
    var v: f32;

    if (standard == 0u) {
        // Rec. 601
        y = 0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b;
        u = (rgb.b - y) / 1.772;
        v = (rgb.r - y) / 1.402;
    } else {
        // Rec. 709
        y = 0.2126 * rgb.r + 0.7152 * rgb.g + 0.0722 * rgb.b;
        u = (rgb.b - y) / 1.8556;
        v = (rgb.r - y) / 1.5748;
    }

    return vec3<f32>(y, u, v);
}

fn encode(yuv: vec3<f32>, range: u32) -> vec3<f32> {
    if (range == 0u) {
        return vec3<f32>(
            16.0 / 255.0 + yuv.x * (219.0 / 255.0),
            128.0 / 255.0 + yuv.y * (224.0 / 255.0),
            128.0 / 255.0 + yuv.z * (224.0 / 255.0),
        );
    }
    return yuv;
}

@fragment
fn fs_main(in: VertOut) -> @location(0) vec4<f32> {
    let dims = textureDimensions(source);
    let pixel = vec2<i32>(floor(in.position.xy));
    let x0 = clamp(pixel.x * 2, 0, i32(dims.x) - 1);
    let x1 = min(x0 + 1, i32(dims.x) - 1);
    let y = clamp(pixel.y, 0, i32(dims.y) - 1);

    let standard = conv.mode & 0xFFu;
    let range = (conv.mode >> 8u) & 0xFFu;
    let yuv0 = encode(rgb_to_yuv(textureLoad(source, vec2<i32>(x0, y), 0).rgb, standard), range);
    let yuv1 = encode(rgb_to_yuv(textureLoad(source, vec2<i32>(x1, y), 0).rgb, standard), range);

    let u = (yuv0.y + yuv1.y) * 0.5;
    let v = (yuv0.z + yuv1.z) * 0.5;
    return vec4<f32>(u, yuv0.x, v, yuv1.x);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct YuvVert {
    position: [f32; 2],
    uv: [f32; 2],
}

const INDICES: [u16; 6] = [0, 1, 2, 2, 1, 3];

/// BGRA/RGBA to limited-range UYVY, packed two pixels per half-width RGBA8 texel.
pub struct YuvConverter {
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
}

impl YuvConverter {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv-convert"),
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
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv-convert"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yuv-convert"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yuv-convert"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<YuvVert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
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
            label: Some("yuv-convert-vb"),
            size: std::mem::size_of::<[YuvVert; 4]>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vertices = [
            YuvVert {
                position: [-1.0, -1.0],
                uv: [0.0, 1.0],
            },
            YuvVert {
                position: [1.0, -1.0],
                uv: [1.0, 1.0],
            },
            YuvVert {
                position: [-1.0, 1.0],
                uv: [0.0, 0.0],
            },
            YuvVert {
                position: [1.0, 1.0],
                uv: [1.0, 0.0],
            },
        ];
        queue.write_buffer(&vb, 0, bytemuck::cast_slice(&vertices));

        let ib = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv-convert-ib"),
            size: std::mem::size_of_val(&INDICES) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ib, 0, bytemuck::cast_slice(&INDICES));

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv-convert-uniform"),
            size: std::mem::size_of::<ConvUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        return Self {
            pipeline,
            bind_layout,
            uniform,
            vb,
            ib,
        };
    }

    /// Convert `source` into `target`. `target.layout` must be the UYVY layout
    /// of the same frame size, at half width.
    #[allow(clippy::too_many_arguments)]
    pub fn convert(
        &self,
        source: &wgpu::TextureView,
        source_layout: &TextureLayout,
        target: &TextureTarget,
        color: ColorSpace,
        range: ColorRange,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let uniform = ConvUniform {
            mode: color.code() | (range.code() << 8),
            width: source_layout.width as f32,
            height: source_layout.height as f32,
            _pad: 0.0,
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv-convert"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.uniform.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("yuv-convert"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("yuv-convert"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
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
            rpass.set_pipeline(&self.pipeline);
            rpass.set_vertex_buffer(0, self.vb.slice(..));
            rpass.set_index_buffer(self.ib.slice(..), wgpu::IndexFormat::Uint16);
            rpass.set_bind_group(0, &bind_group, &[]);
            rpass.draw_indexed(0..6, 0, 0..1);
        }
        queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::SHADER;

    fn encode(rgb: (f32, f32, f32), rec601: bool) -> (f32, f32, f32) {
        let (r, g, b) = rgb;
        let (y, u, v) = if rec601 {
            (
                0.299 * r + 0.587 * g + 0.114 * b,
                (b - (0.299 * r + 0.587 * g + 0.114 * b)) / 1.772,
                (r - (0.299 * r + 0.587 * g + 0.114 * b)) / 1.402,
            )
        } else {
            (
                0.2126 * r + 0.7152 * g + 0.0722 * b,
                (b - (0.2126 * r + 0.7152 * g + 0.0722 * b)) / 1.8556,
                (r - (0.2126 * r + 0.7152 * g + 0.0722 * b)) / 1.5748,
            )
        };
        return (
            16.0 / 255.0 + y * (219.0 / 255.0),
            128.0 / 255.0 + u * (224.0 / 255.0),
            128.0 / 255.0 + v * (224.0 / 255.0),
        );
    }

    fn decode(yuv: (f32, f32, f32), rec601: bool) -> (f32, f32, f32) {
        let (y8, u8, v8) = yuv;
        let y = y8 - 16.0 / 255.0;
        let u = u8 - 128.0 / 255.0;
        let v = v8 - 128.0 / 255.0;
        let (r, g, b) = if rec601 {
            (
                1.164 * y + 1.596 * v,
                1.164 * y - 0.391 * u - 0.813 * v,
                1.164 * y + 2.018 * u,
            )
        } else {
            (
                1.164 * y + 1.793 * v,
                1.164 * y - 0.213 * u - 0.533 * v,
                1.164 * y + 2.112 * u,
            )
        };
        return (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
    }

    fn assert_close(actual: (f32, f32, f32), expected: (f32, f32, f32)) {
        let error = (actual.0 - expected.0)
            .abs()
            .max((actual.1 - expected.1).abs())
            .max((actual.2 - expected.2).abs());
        assert!(error < 0.01, "actual {actual:?}, expected {expected:?}, error {error}");
    }

    #[test]
    fn limited_yuv_round_trip_uses_full_chroma_amplitude() {
        let colors = [(0.75, 0.0, 0.0), (0.0, 0.75, 0.0), (0.0, 0.0, 0.75), (0.75, 0.75, 0.75)];
        for color in colors {
            for rec601 in [false, true] {
                let yuv = encode(color, rec601);
                assert!((0.0..=1.0).contains(&yuv.1));
                assert!((0.0..=1.0).contains(&yuv.2));
                assert_close(decode(yuv, rec601), color);
            }
        }
    }

    #[test]
    fn shader_parses_and_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(SHADER).expect("WGSL failed to parse");
        let mut validator = wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("WGSL failed to validate");
    }
}
