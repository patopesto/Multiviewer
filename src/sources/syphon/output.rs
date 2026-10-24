use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_metal::{
    MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDevice,
    MTLOrigin, MTLSize, MTLTexture,
};
use serde::{Deserialize, Serialize};
use syphon_core::SyphonServer;
use syphon_metal::wgpu_interop::{self, SharedEvent};
use syphon_metal::{IOSurfacePool, MetalContext};

use super::super::{OutputStats, Protocol};
use super::super::output::{OutputId, VideoOutput};

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
// @docs protocol="Syphon" direction=output
pub struct SyphonOutputConfig {
    /// Name of the announced Syphon server. Empty uses the output's name.
    // @docs hidden
    pub server_name: String,
}

enum PublishOutcome {
    ZeroCopy,
    PoolExhausted,
    Failed,
}

/// Syphon server plus the Metal machinery to publish into it.
///
/// The blit runs on Syphon's own `MTLCommandQueue`, so it is ordered against
/// wgpu's flip render with an `MTLSharedEvent` instead of a CPU drain: the flip
/// submit raises the event and the blit waits for it on the GPU.
struct SyphonPublisher {
    server: SyphonServer,
    metal: MetalContext,
    pool: IOSurfacePool,
    event: Option<SharedEvent>,
    value: u64,
    width: u32,
    height: u32,
}

// SAFETY: `SyphonMetalServer`, Metal device/queue objects and IOSurfaces are
// documented thread-safe; this value only lives behind `SyphonOutput::output`'s
// mutex, so it is never touched concurrently.
unsafe impl Send for SyphonPublisher {}

impl SyphonPublisher {
    fn new(name: &str, device: &wgpu::Device, width: u32, height: u32) -> Option<Self> {
        let metal = MetalContext::from_wgpu_device(device)?;
        let device_ptr =
            metal.device() as *const ProtocolObject<dyn MTLDevice> as *mut AnyObject;
        let server =
            SyphonServer::new_with_name_and_device(name, device_ptr, width, height).ok()?;
        let event = wgpu_interop::new_shared_event(device);
        if event.is_none() {
            tracing::warn!(
                output = name,
                "Syphon shared event unavailable; falling back to a GPU drain per publish"
            );
        }
        return Some(Self {
            server,
            metal,
            pool: IOSurfacePool::new(width, height, 3),
            event,
            value: 0,
            width,
            height,
        });
    }

    /// Monotonic shared-event value. Waits are `>=`, so a skipped value is
    /// harmless as long as each publish waits on the value it staged.
    fn next_value(&mut self) -> u64 {
        self.value += 1;
        return self.value;
    }

    /// Blit `flipped` into a pooled IOSurface and hand it to Syphon.
    ///
    /// `wait` is the event value the flip submit signalled; `None` means no
    /// fence was staged, so fall back to the CPU drain before the blit.
    fn publish(
        &mut self,
        flipped: &wgpu::Texture,
        device: &wgpu::Device,
        wait: Option<u64>,
    ) -> PublishOutcome {
        let Some(surface) = self.pool.acquire() else {
            return PublishOutcome::PoolExhausted;
        };

        if wait.is_none() {
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }

        let (width, height) = (self.width, self.height);
        let mut published = false;
        objc2::rc::autoreleasepool(|_| {
            wgpu_interop::with_metal_texture(flipped, |src| {
                let Some(src) = src else { return };
                let Some(dest) = self.metal.create_texture_from_iosurface(&surface, width, height)
                else {
                    return;
                };
                let Some(cmd) = self.metal.queue().commandBuffer() else {
                    return;
                };
                // Waits and signals are encoded outside any encoder.
                if let (Some(value), Some(event)) = (wait, self.event.as_ref()) {
                    cmd.encodeWaitForEvent_value(ProtocolObject::from_ref(&**event), value);
                }
                let Some(enc) = cmd.blitCommandEncoder() else {
                    return;
                };
                unsafe {
                    enc.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                        src,
                        0, // source slice
                        0, // source level
                        MTLOrigin { x: 0, y: 0, z: 0 },
                        MTLSize {
                            width: width as usize,
                            height: height as usize,
                            depth: 1,
                        },
                        &dest,
                        0, // destination slice
                        0, // destination level
                        MTLOrigin { x: 0, y: 0, z: 0 },
                    );
                }
                enc.endEncoding();

                // Syphon references the command buffer and the texture before
                // the commit, so the frame is on the GPU when it is announced.
                let tex_ptr =
                    &*dest as *const ProtocolObject<dyn MTLTexture> as *mut AnyObject;
                let cmd_ptr =
                    &*cmd as *const ProtocolObject<dyn MTLCommandBuffer> as *mut AnyObject;
                unsafe { self.server.publish_metal_texture(tex_ptr, cmd_ptr) };
                cmd.commit();
                published = true;
            });
        });

        // Triple-buffering assumes the GPU is done with a surface by the time it
        // comes around again; releasing immediately keeps the pool free.
        self.pool.release(surface);

        return if published {
            PublishOutcome::ZeroCopy
        } else {
            PublishOutcome::Failed
        };
    }
}

pub struct SyphonOutput {
    name: String,
    config: SyphonOutputConfig,
    output: Mutex<Option<SyphonPublisher>>,
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
    pub fn new(_id: OutputId, name: String, config: SyphonOutputConfig, enabled: bool) -> Self {
        Self {
            name,
            config,
            output: Mutex::new(None),
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
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<FlipVert>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    })],
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

    /// Render the vertical flip. When `signal` is given, the event is staged so
    /// the submit raises it once the flip has been drawn; returns whether the
    /// submit is fenced. `None` means no flip was submitted.
    fn render_flip(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        signal: Option<(&SharedEvent, u64)>,
    ) -> Option<bool> {
        let vb = self.flip_vb.get().unwrap();
        queue.write_buffer(vb, 0, bytemuck::cast_slice(&FLIP_VERTICES));

        let view = self.flipped_view.lock().unwrap();
        let Some(ref view) = *view else { return None };
        let bind_group = self.flipped_bind_group.lock().unwrap();
        let Some(ref bind_group) = *bind_group else {
            return None;
        };

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

        // Stage right before the submit: the signal attaches to this command
        // buffer and fires when the flip has landed.
        let mut signalled = false;
        if let Some((event, value)) = signal {
            signalled = wgpu_interop::queue_signal_event(queue, event, value);
        }
        queue.submit(Some(encoder.finish()));
        return Some(signalled);
    }
}

impl VideoOutput for SyphonOutput {
    fn present(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        if !self.enabled.load(Ordering::Relaxed) {
            return false;
        }

        self.ensure_flip_resources(device, queue);

        let mut output_guard = self.output.lock().unwrap();
        let stale = output_guard
            .as_ref()
            .map(|p| p.width != width || p.height != height)
            .unwrap_or(true);
        if stale {
            let server_name = if self.config.server_name.is_empty() {
                self.name.as_str()
            } else {
                self.config.server_name.as_str()
            };
            let Some(publisher) = SyphonPublisher::new(server_name, device, width, height) else {
                tracing::error!(output=self.name, "Syphon output creation failed");
                return false;
            };
            tracing::info!(output=self.name, "Syphon output created: ({}x{})", width, height);
            {
                let mut s = self.stats.lock().unwrap();
                s.width = width;
                s.height = height;
            }
            *output_guard = Some(publisher);
        }

        let Some(publisher) = output_guard.as_mut() else {
            return false;
        };

        // A server with no client would flip and publish into the void every
        // cycle; the server object stays alive so clients can still find it.
        if publisher.server.client_count() == 0 {
            return false;
        }

        let value = publisher.next_value();
        let signal = publisher.event.as_ref().map(|event| (event, value));
        let signalled = {
            let flip_span = tracing::debug_span!("flip");
            let _flip_guard = flip_span.entered();
            self.ensure_flipped_texture(device, width, height, texture);
            self.render_flip(device, queue, width, height, signal)
        };
        let Some(signalled) = signalled else {
            return false;
        };

        let publish_span = tracing::debug_span!("publish");
        let _publish_guard = publish_span.entered();

        let flipped_texture = self.flipped_texture.lock().unwrap();
        let Some(ref flipped) = *flipped_texture else {
            return false;
        };

        let start = std::time::Instant::now();
        let outcome = publisher.publish(
            flipped,
            device,
            if signalled { Some(value) } else { None },
        );
        let elapsed = start.elapsed().as_secs_f32() * 1000.0;

        tracing::trace!(output=self.name, "Syphon publish: elapsed {:.2}ms", elapsed);

        let mut s = self.stats.lock().unwrap();
        s.send_time_ms = elapsed;
        match outcome {
            PublishOutcome::ZeroCopy => s.record_sent(),
            PublishOutcome::PoolExhausted | PublishOutcome::Failed => s.frames_dropped += 1,
        }

        // No CPU readback: the frame is published zero-copy inside present.
        return false;
    }

    fn name(&self) -> &str {
        return &self.name;
    }

    fn enabled(&self) -> bool {
        return self.enabled.load(Ordering::Relaxed);
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn stats(&self) -> Arc<Mutex<OutputStats>> {
        return self.stats.clone();
    }

    fn protocol(&self) -> Protocol {
        return Protocol::Syphon;
    }
}
