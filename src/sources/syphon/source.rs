use super::super::{Frame, VideoSource};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}};

#[derive(Clone)]
pub struct SyphonConfig {}

impl Default for SyphonConfig {
    fn default() -> Self {
        Self {}
    }
}

/// GPU-only Syphon receiver.
pub struct SyphonSource {
    name: String,
    server_name: String,
    input: Mutex<Option<syphon_wgpu::SyphonWgpuInput>>,
    layout: Mutex<Option<Arc<wgpu::BindGroupLayout>>>,
    sampler: Mutex<Option<Arc<wgpu::Sampler>>>,
    bg: Mutex<Option<Arc<wgpu::BindGroup>>>,
    dims: Mutex<(u32, u32)>,
    seq: AtomicU64,
}

impl SyphonSource {
    pub fn spawn(name: String, server_name: String) -> Self {
        Self {
            name,
            server_name,
            input: Mutex::new(None),
            layout: Mutex::new(None),
            sampler: Mutex::new(None),
            bg: Mutex::new(None),
            dims: Mutex::new((0, 0)),
            seq: AtomicU64::new(0),
        }
    }
}

impl VideoSource for SyphonSource {
    fn latest(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Frame> {
        let mut input_guard = self.input.lock().unwrap();

        if input_guard.is_none() {
            let mut input = syphon_wgpu::SyphonWgpuInput::new(device, queue);
            match input.connect(&self.server_name) {
                Ok(()) => {
                    tracing::info!("Syphon connected to {}", self.server_name);
                }
                Err(e) => {
                    tracing::debug!("Syphon connect failed for {}: {}", self.server_name, e);
                    return None;
                }
            }
            *input_guard = Some(input);
        }

        let input = input_guard.as_mut().unwrap();

        if input.receive_texture(device, queue) {
            let tex = input.output_texture()?;
            let size = tex.size();
            let w = size.width;
            let h = size.height;

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
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&self.name),
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
                    ],
                });
                *bg = Some(Arc::new(bind_group));
                *dims = (w, h);
            }

            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            Some(Frame::Syphon(super::super::SyphonFrame {
                bg: bg.as_ref().unwrap().clone(),
                w,
                h,
                seq,
            }))
        } else {
            // Return the cached frame even if no new frame arrived
            let bg = self.bg.lock().unwrap();
            let dims = self.dims.lock().unwrap();
            if let Some(ref bind_group) = *bg {
                let seq = self.seq.load(Ordering::Relaxed);
                Some(Frame::Syphon(super::super::SyphonFrame {
                    bg: bind_group.clone(),
                    w: dims.0,
                    h: dims.1,
                    seq,
                }))
            } else {
                None
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}
