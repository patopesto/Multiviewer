use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

use multiviewer_decklink::{
    decklink_output_free, decklink_output_new, decklink_output_present_frame,
    decklink_output_start, decklink_output_stop, DisplayMode,
};

use super::super::{OutputStats, Protocol};
use super::super::output::{OutputId, VideoOutput};

const SCALE_SHADER: &str = r#"
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
    return textureSample(canvas_texture, canvas_sampler, in.uv);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ScaleVert {
    position: [f32; 2],
    uv: [f32; 2],
}

const SCALE_INDICES: [u16; 6] = [0, 1, 2, 2, 1, 3];

#[derive(Serialize, Deserialize, Clone)]
pub struct DecklinkOutputConfig {
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub display_mode: DisplayMode,
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_fps")]
    pub fps: f64,
}

fn default_fps() -> f64 {
    60.0
}

impl Default for DecklinkOutputConfig {
    fn default() -> Self {
        Self {
            device_name: String::new(),
            display_mode: DisplayMode::Hd1080p6000,
            width: 1920,
            height: 1080,
            fps: 60.0,
        }
    }
}

pub struct DecklinkOutput {
    id: OutputId,
    name: String,
    config: DecklinkOutputConfig,
    enabled: AtomicBool,
    width: AtomicU32,
    height: AtomicU32,
    stats: Arc<Mutex<OutputStats>>,
    frame_tx: Option<mpsc::SyncSender<Vec<u8>>>,
    pending: Arc<AtomicBool>,
    readback: Mutex<Option<wgpu::Buffer>>,
    thread: Option<thread::JoinHandle<()>>,
    scale_bind_layout: OnceLock<wgpu::BindGroupLayout>,
    scale_sampler: OnceLock<wgpu::Sampler>,
    scale_pipeline: OnceLock<wgpu::RenderPipeline>,
    scale_vb: OnceLock<wgpu::Buffer>,
    scale_ib: OnceLock<wgpu::Buffer>,
    scaled_texture: Mutex<Option<wgpu::Texture>>,
    scaled_view: Mutex<Option<wgpu::TextureView>>,
    scaled_bind_group: Mutex<Option<wgpu::BindGroup>>,
}

impl DecklinkOutput {
    pub fn new(id: OutputId, name: String, config: DecklinkOutputConfig, enabled: bool) -> Self {
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let stats = Arc::new(Mutex::new(OutputStats::default()));
        let stats_clone = Arc::clone(&stats);
        let pending = Arc::new(AtomicBool::new(false));
        let pending_t = Arc::clone(&pending);

        let thread_name = name.clone();
        let thread_config = config.clone();
        let thread_fps = config.fps;
        let thread = thread::Builder::new()
            .name(format!("decklink-out-{id}"))
            .spawn(move || {
                let device_name = thread_config.device_name.clone();
                let mode_id: u32 = thread_config.display_mode.into();
                let Ok(device_name_c) = CString::new(device_name) else {
                    tracing::error!(output=thread_name, "DeckLink output device name contains null");
                    return;
                };

                let output = unsafe { decklink_output_new(device_name_c.as_ptr()) };
                if output.is_null() {
                    tracing::error!(output=thread_name, "DeckLink output failed to create output handle");
                    return;
                }

                let frame_interval = if thread_fps > 0.0 {
                    Duration::from_secs_f64(1.0 / thread_fps)
                } else {
                    Duration::from_secs_f64(1.0 / 60.0)
                };
                let mut last_frame_time = Instant::now();

                let mut started = false;
                while let Ok(buffer) = frame_rx.recv() {
                    pending_t.store(false, Ordering::Release);
                    if buffer.len() < 16 {
                        continue;
                    }
                    let frame_w =
                        u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
                    let frame_h =
                        u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
                    let expected = (frame_w * frame_h * 4) as usize;
                    if buffer.len() < expected + 16 {
                        continue;
                    }

                    if !started {
                        if unsafe { decklink_output_start(output, mode_id) } {
                            started = true;
                            tracing::info!(output=thread_name, "DeckLink output started on {}", mode_id);
                        } else {
                            tracing::error!(output=thread_name, "DeckLink output start failed");
                            break;
                        }
                    }

                    {
                        let pacing_span = tracing::debug_span!("decklink_pacing");
                        let _pacing_guard = pacing_span.entered();
                        let now = Instant::now();
                        let elapsed = now.duration_since(last_frame_time);
                        if elapsed < frame_interval {
                            std::thread::sleep(frame_interval - elapsed);
                        }
                        last_frame_time = Instant::now();
                    }

                    let send_span = tracing::debug_span!("decklink_send");
                    let _send_guard = send_span.entered();
                    let start = Instant::now();
                    let ok = unsafe {
                        decklink_output_present_frame(
                            output,
                            buffer.as_ptr().add(16),
                            frame_w as i32,
                            frame_h as i32,
                            (frame_w * 4) as i32,
                        )
                    };

                    {
                        let mut s = stats_clone.lock().unwrap();
                        s.width = frame_w;
                        s.height = frame_h;
                        s.send_time_ms = start.elapsed().as_secs_f32() * 1000.0;
                        if ok {
                            s.record_sent();
                        } else {
                            s.frames_dropped += 1;
                        }
                    }
                }

                if started {
                    unsafe { decklink_output_stop(output) };
                }
                unsafe { decklink_output_free(output) };
            })
            .expect("spawn decklink output thread");

        Self {
            id,
            name,
            config,
            enabled: AtomicBool::new(enabled),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            stats,
            frame_tx: Some(frame_tx),
            pending,
            readback: Mutex::new(None),
            thread: Some(thread),
            scale_bind_layout: OnceLock::new(),
            scale_sampler: OnceLock::new(),
            scale_pipeline: OnceLock::new(),
            scale_vb: OnceLock::new(),
            scale_ib: OnceLock::new(),
            scaled_texture: Mutex::new(None),
            scaled_view: Mutex::new(None),
            scaled_bind_group: Mutex::new(None),
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
        if self.pending.load(Ordering::Acquire) {
            return;
        }

        let out_w = self.config.width;
        let out_h = self.config.height;
        if out_w == 0 || out_h == 0 {
            return;
        }

        self.width.store(out_w, Ordering::Relaxed);
        self.height.store(out_h, Ordering::Relaxed);

        {
            let scale_span = tracing::debug_span!("scale");
            let _scale_guard = scale_span.entered();
            self.ensure_scale_resources(device, queue);
            self.ensure_scaled_texture(device, out_w, out_h, texture);
            self.render_scale(device, queue, out_w, out_h, width, height);
        }

        let bytes_per_row = out_w * 4;
        let aligned_bytes_per_row = (bytes_per_row + 255) & !255;
        let buffer_size = (aligned_bytes_per_row * out_h) as u64;
        let mut readback_slot = self.readback.lock().unwrap();
        if readback_slot.as_ref().is_none_or(|b| b.size() != buffer_size + 16) {
            *readback_slot = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("decklink-output-readback"),
                size: buffer_size + 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));
        }
        let readback_buffer = readback_slot.as_ref().unwrap();

        {
            let readback_span = tracing::debug_span!("readback");
            let _readback_guard = readback_span.entered();
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("decklink-output-readback"),
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: self.scaled_texture.lock().unwrap().as_ref().unwrap(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: readback_buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 16,
                        bytes_per_row: Some(aligned_bytes_per_row),
                        rows_per_image: Some(out_h),
                    },
                },
                wgpu::Extent3d {
                    width: out_w,
                    height: out_h,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit(Some(encoder.finish()));
        }

        let wait_span = tracing::debug_span!("readback_wait");
        let _wait_guard = wait_span.entered();
        let buffer_slice = readback_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), wgpu::BufferAsyncError>>();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
            tracing::error!(output=self.id, "DeckLink output poll failed");
            return;
        }
        if rx.recv().unwrap().is_err() {
            tracing::error!(output=self.id, "DeckLink output readback map failed");
            return;
        }

        let view = buffer_slice.get_mapped_range();
        let row = (out_w * 4) as usize;
        let aligned = (row + 255) & !255;
        let mut buffer = Vec::with_capacity(16 + row * out_h as usize);
        buffer.extend_from_slice(&[0u8; 16]);
        if aligned == row {
            buffer.extend_from_slice(&view[16..16 + row * out_h as usize]);
        } else {
            for y in 0..out_h as usize {
                let src_start = 16 + y * aligned;
                buffer.extend_from_slice(&view[src_start..src_start + row]);
            }
        }
        drop(view);
        readback_buffer.unmap();

        buffer[0..4].copy_from_slice(&out_w.to_le_bytes());
        buffer[4..8].copy_from_slice(&out_h.to_le_bytes());
        let mode_id: u32 = self.config.display_mode.into();
        buffer[8..12].copy_from_slice(&mode_id.to_le_bytes());

        let sent = match self.frame_tx.as_ref() {
            Some(tx) => tx.try_send(buffer).is_ok(),
            None => false,
        };
        if sent {
            self.pending.store(true, Ordering::Release);
        } else {
            let mut s = self.stats.lock().unwrap();
            s.frames_dropped += 1;
        }
    }

    fn ensure_scale_resources(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let _ = self.scale_bind_layout.get_or_init(|| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("decklink-scale"),
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

        let _ = self.scale_sampler.get_or_init(|| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            })
        });

        let _ = self.scale_pipeline.get_or_init(|| {
            let bind_layout = self.scale_bind_layout.get().unwrap();
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("decklink-scale"),
                source: wgpu::ShaderSource::Wgsl(SCALE_SHADER.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("decklink-scale"),
                bind_group_layouts: &[Some(bind_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("decklink-scale"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ScaleVert>() as u64,
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

        let _ = self.scale_vb.get_or_init(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("decklink-scale-vb"),
                size: std::mem::size_of::<[ScaleVert; 4]>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });

        let _ = self.scale_ib.get_or_init(|| {
            let ib = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("decklink-scale-ib"),
                size: std::mem::size_of_val(&SCALE_INDICES) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&ib, 0, bytemuck::cast_slice(&SCALE_INDICES));
            ib
        });
    }

    fn ensure_scaled_texture(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        texture: &wgpu::Texture,
    ) {
        let mut scaled_texture = self.scaled_texture.lock().unwrap();
        let recreate = scaled_texture
            .as_ref()
            .map(|t| t.width() != width || t.height() != height)
            .unwrap_or(true);

        if recreate {
            let new_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("decklink-scaled"),
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
                label: Some("decklink-scale"),
                layout: self.scale_bind_layout.get().unwrap(),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &texture.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(
                            self.scale_sampler.get().unwrap(),
                        ),
                    },
                ],
            });

            *scaled_texture = Some(new_texture);
            *self.scaled_view.lock().unwrap() = Some(view);
            *self.scaled_bind_group.lock().unwrap() = Some(bind_group);
        }
    }

    fn render_scale(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        out_w: u32,
        out_h: u32,
        canvas_w: u32,
        canvas_h: u32,
    ) {
        let canvas_aspect = canvas_w as f32 / canvas_h.max(1) as f32;
        let output_aspect = out_w as f32 / out_h.max(1) as f32;

        let (x1, x2, y1, y2) = if output_aspect > canvas_aspect {
            let scaled_w = out_h as f32 * canvas_aspect;
            let x_off = (out_w as f32 - scaled_w) / 2.0;
            let x1 = (x_off / out_w as f32) * 2.0 - 1.0;
            let x2 = ((x_off + scaled_w) / out_w as f32) * 2.0 - 1.0;
            (x1, x2, -1.0f32, 1.0f32)
        } else {
            let scaled_h = out_w as f32 / canvas_aspect;
            let y_off = (out_h as f32 - scaled_h) / 2.0;
            let y1 = (y_off / out_h as f32) * 2.0 - 1.0;
            let y2 = ((y_off + scaled_h) / out_h as f32) * 2.0 - 1.0;
            (-1.0f32, 1.0f32, y1, y2)
        };

        let vertices = [
            ScaleVert {
                position: [x1, y1],
                uv: [0.0, 1.0],
            },
            ScaleVert {
                position: [x2, y1],
                uv: [1.0, 1.0],
            },
            ScaleVert {
                position: [x1, y2],
                uv: [0.0, 0.0],
            },
            ScaleVert {
                position: [x2, y2],
                uv: [1.0, 0.0],
            },
        ];
        queue.write_buffer(self.scale_vb.get().unwrap(), 0, bytemuck::cast_slice(&vertices));

        let view = self.scaled_view.lock().unwrap();
        let Some(ref view) = *view else { return };
        let bind_group = self.scaled_bind_group.lock().unwrap();
        let Some(ref bind_group) = *bind_group else { return };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("decklink-scale"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("decklink-scale"),
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
            rpass.set_viewport(0.0, 0.0, out_w as f32, out_h as f32, 0.0, 1.0);
            rpass.set_pipeline(self.scale_pipeline.get().unwrap());
            rpass.set_vertex_buffer(0, self.scale_vb.get().unwrap().slice(..));
            rpass.set_index_buffer(
                self.scale_ib.get().unwrap().slice(..),
                wgpu::IndexFormat::Uint16,
            );
            rpass.set_bind_group(0, bind_group, &[]);
            rpass.draw_indexed(0..6, 0, 0..1);
        }
        queue.submit(Some(encoder.finish()));
    }

    #[allow(dead_code)]
    pub fn config(&self) -> &DecklinkOutputConfig {
        &self.config
    }
}

impl VideoOutput for DecklinkOutput {
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

    fn busy(&self) -> bool {
        return self.pending.load(Ordering::Acquire);
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn stats(&self) -> Arc<Mutex<OutputStats>> {
        Arc::clone(&self.stats)
    }

    fn protocol(&self) -> Protocol {
        Protocol::Decklink
    }
}

impl Drop for DecklinkOutput {
    fn drop(&mut self) {
        drop(self.frame_tx.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
