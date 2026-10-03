use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use serde::{Deserialize, Serialize};

use super::super::Protocol;
use super::super::output::{OutputId, OutputStats, VideoOutput};

const FLIP_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var canvas_texture: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(canvas_texture, canvas_sampler, vec2<f32>(in.uv.x, in.uv.y));
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FlipVert {
    position: [f32; 2],
    uv: [f32; 2],
}

const FLIP_VERTICES: [FlipVert; 4] = [
    FlipVert {
        position: [-1.0, -1.0],
        uv: [0.0, 0.0],
    },
    FlipVert {
        position: [1.0, -1.0],
        uv: [1.0, 0.0],
    },
    FlipVert {
        position: [-1.0, 1.0],
        uv: [0.0, 1.0],
    },
    FlipVert {
        position: [1.0, 1.0],
        uv: [1.0, 1.0],
    },
];

const FLIP_INDICES: [u16; 6] = [0, 1, 2, 2, 1, 3];

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SyphonOutputConfig {
    #[serde(default)]
    pub server_name: String,
}

pub struct SyphonOutput {
    #[allow(dead_code)]
    id: OutputId,
    name: String,
    config: SyphonOutputConfig,
    output: Mutex<Option<syphon_wgpu::SyphonWgpuOutput>>,
    width: AtomicU32,
    height: AtomicU32,
    enabled: AtomicBool,
    stats: Arc<Mutex<OutputStats>>,
    // Vertical flip resources. Created lazily on first present().
    flip_bind_layout: OnceLock<wgpu::BindGroupLayout>,
    flip_sampler: OnceLock<wgpu::Sampler>,
    flip_pipeline: OnceLock<wgpu::RenderPipeline>,
    flip_vb: OnceLock<wgpu::Buffer>,
    flip_ib: OnceLock<wgpu::Buffer>,
    flipped_texture: Mutex<Option<wgpu::Texture>>,
    flipped_view: Mutex<Option<wgpu::TextureView>>,
    flipped_bind_group: Mutex<Option<wgpu::BindGroup>>,
}

impl SyphonOutput {
    pub fn new(id: OutputId, name: String, config: SyphonOutputConfig, enabled: bool) -> Self {
        Self {
            id,
            name,
            config,
            output: Mutex::new(None),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            enabled: AtomicBool::new(enabled),
            stats: Arc::new(Mutex::new(OutputStats::default())),
            flip_bind_layout: OnceLock::new(),
            flip_sampler: OnceLock::new(),
            flip_pipeline: OnceLock::new(),
            flip_vb: OnceLock::new(),
            flip_ib: OnceLock::new(),
            flipped_texture: Mutex::new(None),
            flipped_view: Mutex::new(None),
            flipped_bind_group: Mutex::new(None),
        }
    }

    pub fn present(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        self.ensure_flip_resources(device, queue);

        let mut output_guard = self.output.lock().unwrap();
        let current_width = self.width.load(Ordering::Relaxed);
        let current_height = self.height.load(Ordering::Relaxed);

        if output_guard.is_none() || current_width != width || current_height != height {
            match syphon_wgpu::SyphonWgpuOutput::new(&self.config.server_name, device, queue, width, height) {
                Ok(output) => {
                    tracing::info!(output=self.name, "Syphon output created: ({}x{})", width, height);
                    self.width.store(width, Ordering::Relaxed);
                    self.height.store(height, Ordering::Relaxed);
                    *output_guard = Some(output);
                    drop(output_guard);
                    let mut s = self.stats.lock().unwrap();
                    s.width = width;
                    s.height = height;
                    output_guard = self.output.lock().unwrap();
                }
                Err(e) => {
                    tracing::error!(output=self.name, "Syphon output creation failed: {}", e);
                    return;
                }
            }
        }

        // A server with no client would flip and publish into the void every
        // cycle; the server object stays alive so clients can still find it.
        if output_guard.as_ref().map(|o| o.client_count()).unwrap_or(0) == 0 {
            return;
        }

        // Recreate the flipped texture/bind group if dimensions changed.
        {
            let flip_span = tracing::debug_span!("flip");
            let _flip_guard = flip_span.entered();
            self.ensure_flipped_texture(device, width, height, texture);
            self.render_flip(device, queue, width, height);
        }

        let publish_span = tracing::debug_span!("publish");
        let _publish_guard = publish_span.entered();
        if let Some(ref mut output) = *output_guard {
            let _flipped_view = self.flipped_view.lock().unwrap();
            let Some(ref _view) = *_flipped_view else {
                return;
            };

            let clients = output.client_count();
            let start = std::time::Instant::now();

            // syphon_wgpu::publish expects a &wgpu::Texture, not a view.
            // We access the texture stored in flipped_texture.
            let flipped_texture = self.flipped_texture.lock().unwrap();
            let Some(ref flipped) = *flipped_texture else {
                return;
            };

            let status = output.publish(flipped, device, queue);
            let elapsed = start.elapsed().as_secs_f32() * 1000.0;

            tracing::trace!(output=self.name, "Syphon publish: {:?}, clients: {}, elapsed: {:.2}ms", status, clients, elapsed);

            let mut s = self.stats.lock().unwrap();
            s.send_time_ms = elapsed;
            match status {
                syphon_wgpu::PublishStatus::ZeroCopy | syphon_wgpu::PublishStatus::CpuFallback => {
                    s.frames_sent += 1;
                }
                syphon_wgpu::PublishStatus::NoClients
                | syphon_wgpu::PublishStatus::PoolExhausted => {
                    s.frames_dropped += 1;
                }
            }
        }
    }

    fn ensure_flip_resources(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let _ = self.flip_bind_layout.get_or_init(|| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("syphon-flip"),
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
            })
        });

        let _ = self.flip_sampler.get_or_init(|| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            })
        });

        let _ = self.flip_pipeline.get_or_init(|| {
            let bind_layout = self.flip_bind_layout.get().unwrap();
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("syphon-flip"),
                source: wgpu::ShaderSource::Wgsl(FLIP_SHADER.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("syphon-flip"),
                bind_group_layouts: &[Some(bind_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("syphon-flip"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<FlipVert>() as u64,
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
            })
        });

        let _ = self.flip_vb.get_or_init(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("syphon-flip-vb"),
                size: std::mem::size_of_val(&FLIP_VERTICES) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });

        let _ = self.flip_ib.get_or_init(|| {
            let ib = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("syphon-flip-ib"),
                size: std::mem::size_of_val(&FLIP_INDICES) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&ib, 0, bytemuck::cast_slice(&FLIP_INDICES));
            ib
        });
    }

    fn ensure_flipped_texture(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        texture: &wgpu::Texture,
    ) {
        let mut flipped_texture = self.flipped_texture.lock().unwrap();
        let recreate = flipped_texture
            .as_ref()
            .map(|t| t.width() != width || t.height() != height)
            .unwrap_or(true);

        if recreate {
            let new_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("syphon-flipped"),
                size: wgpu::Extent3d {
                    width,
                    height,
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
            let view = new_texture.create_view(&Default::default());

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("syphon-flip"),
                layout: self.flip_bind_layout.get().unwrap(),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(self.flip_sampler.get().unwrap()),
                    },
                ],
            });

            *flipped_texture = Some(new_texture);
            *self.flipped_view.lock().unwrap() = Some(view);
            *self.flipped_bind_group.lock().unwrap() = Some(bind_group);
        }
    }

    fn render_flip(&self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) {
        let vb = self.flip_vb.get().unwrap();
        queue.write_buffer(vb, 0, bytemuck::cast_slice(&FLIP_VERTICES));

        let view = self.flipped_view.lock().unwrap();
        let Some(ref view) = *view else { return };
        let bind_group = self.flipped_bind_group.lock().unwrap();
        let Some(ref bind_group) = *bind_group else { return };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("syphon-flip"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("syphon-flip"),
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
            rpass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
            rpass.set_pipeline(self.flip_pipeline.get().unwrap());
            rpass.set_vertex_buffer(0, vb.slice(..));
            rpass.set_index_buffer(self.flip_ib.get().unwrap().slice(..), wgpu::IndexFormat::Uint16);
            rpass.set_bind_group(0, bind_group, &[]);
            rpass.draw_indexed(0..6, 0, 0..1);
        }
        queue.submit(Some(encoder.finish()));
    }
}

#[cfg(target_os = "macos")]
impl VideoOutput for SyphonOutput {
    fn present(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        self.present(texture, width, height, device, queue);
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn stats(&self) -> Arc<Mutex<OutputStats>> {
        self.stats.clone()
    }

    fn protocol(&self) -> Protocol {
        Protocol::Syphon
    }
}
