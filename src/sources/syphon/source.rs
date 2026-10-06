use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};

use super::SyphonServerInfo;
use super::super::{ConvUniform, Frame, GpuFrame, PixelFormat, SourceRef, SourceStats, VideoSource};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SyphonSourceConfig {}

/// GPU-only Syphon receiver.
pub struct SyphonSource {
    source_ref: SourceRef,
    info: SyphonServerInfo,
    input: Mutex<Option<syphon_wgpu::SyphonWgpuInput>>,
    layout: Mutex<Option<Arc<wgpu::BindGroupLayout>>>,
    sampler: Mutex<Option<Arc<wgpu::Sampler>>>,
    bg: Mutex<Option<Arc<wgpu::BindGroup>>>,
    dims: Mutex<(u32, u32)>,
    seq: AtomicU64,
    stats: Arc<Mutex<SourceStats>>,
}

impl SyphonSource {
    pub fn spawn(source_ref: SourceRef, info: SyphonServerInfo) -> Self {
        Self {
            source_ref,
            info,
            input: Mutex::new(None),
            layout: Mutex::new(None),
            sampler: Mutex::new(None),
            bg: Mutex::new(None),
            dims: Mutex::new((0, 0)),
            seq: AtomicU64::new(0),
            stats: Arc::new(Mutex::new(SourceStats::new())),
        }
    }
}

impl VideoSource for SyphonSource {
    fn latest(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Frame> {
        let mut input_guard = self.input.lock().unwrap();

        if input_guard.is_none() {
            let mut input = syphon_wgpu::SyphonWgpuInput::new(device, queue);
            let connected = input
                .connect_by_info(&self.info)
                .or_else(|_| input.connect(&self.source_ref));
            match connected {
                Ok(()) => {
                    tracing::info!(source=self.source_ref, "Syphon connected");
                }
                Err(e) => {
                    tracing::error!(source=self.source_ref, "Syphon connect failed: {}", e);
                    return None;
                }
            }
            *input_guard = Some(input);
        }

        let input = input_guard.as_mut().unwrap();

        return if input.receive_texture(device, queue) {
            let tex = input.output_texture()?;
            let size = tex.size();
            let w = size.width;
            let h = size.height;
            {
                let mut s = self.stats.lock().unwrap();
                s.record_frame(w, h, PixelFormat::Bgra8.label(), 0.0);
                s.record_copy_time(0.0);
            }

            let mut dims = self.dims.lock().unwrap();
            let mut bg = self.bg.lock().unwrap();
            let mut layout = self.layout.lock().unwrap();
            let mut sampler = self.sampler.lock().unwrap();

            if layout.is_none() {
                let l = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("syphon"),
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
                *layout = Some(Arc::new(l));

                let s = device.create_sampler(&wgpu::SamplerDescriptor {
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    address_mode_u: wgpu::AddressMode::ClampToEdge,
                    address_mode_v: wgpu::AddressMode::ClampToEdge,
                    ..Default::default()
                });
                *sampler = Some(Arc::new(s));
            }

            if dims.0 != w || dims.1 != h || bg.is_none() {
                let view = tex.create_view(&Default::default());
                let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("{}-conv", self.source_ref)),
                    size: std::mem::size_of::<ConvUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(
                    &uniform,
                    0,
                    bytemuck::cast_slice(&[ConvUniform {
                        mode: 0,
                        width: w as f32,
                        height: h as f32,
                        _pad: 0.0,
                    }]),
                );
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&self.source_ref),
                    layout: layout.as_ref().unwrap(),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sampler.as_ref().unwrap()),
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
                });
                *bg = Some(Arc::new(bind_group));
                *dims = (w, h);
            }

            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            Some(Frame::Gpu(GpuFrame {
                bg: bg.as_ref().unwrap().clone(),
                w,
                h,
                seq,
                flip_v: true,
            }))
        } else {
            // Return the cached frame even if no new frame arrived
            let bg = self.bg.lock().unwrap();
            let dims = self.dims.lock().unwrap();
            (*bg).as_ref().map(|bind_group| Frame::Gpu(GpuFrame {
                bg: bind_group.clone(),
                w: dims.0,
                h: dims.1,
                seq: self.seq.load(Ordering::Relaxed),
                flip_v: true,
            }))
        };
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }
}
