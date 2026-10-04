use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};
use wgpu::hal::api::Dx12 as Dx12Api;
use windows::core::Interface;
use spout2::dx12::format as Dx12Format;

use super::super::{ConvUniform, Frame, GpuFrame, PixelFormat, SourceRef, SourceStats, VideoSource};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpoutSourceConfig {}

/// Justifies the `unsafe impl Send`: `latest()` touches the receiver only while
/// holding `SpoutSource::receiver`'s mutex, on the render thread.
struct SpoutReceiver(spout2::dx12::Receiver);
unsafe impl Send for SpoutReceiver {}

/// Zero-copy Spout receiver: Spout writes into a wgpu-owned texture via D3D11On12.
///
/// ponytail: assumes Spout's copies order on wgpu's command queue; upgrade path
/// if a driver rejects wgpu's stale barrier state is a fence + `COPY_DEST` barrier.
pub struct SpoutSource {
    source_ref: SourceRef,
    receiver: Mutex<Option<SpoutReceiver>>,
    texture: Mutex<Option<wgpu::Texture>>,
    layout: Mutex<Option<Arc<wgpu::BindGroupLayout>>>,
    sampler: Mutex<Option<Arc<wgpu::Sampler>>>,
    bg: Mutex<Option<Arc<wgpu::BindGroup>>>,
    dims: Mutex<(u32, u32)>,
    seq: AtomicU64,
    stats: Arc<Mutex<SourceStats>>,
    diagnostics_logged: AtomicBool, // Retried every frame; each failure is logged once instead of at 60 Hz.
}

impl SpoutSource {
    pub fn spawn(source_ref: SourceRef) -> Self {
        Self {
            source_ref,
            receiver: Mutex::new(None),
            texture: Mutex::new(None),
            layout: Mutex::new(None),
            sampler: Mutex::new(None),
            bg: Mutex::new(None),
            dims: Mutex::new((0, 0)),
            seq: AtomicU64::new(0),
            stats: Arc::new(Mutex::new(SourceStats::new())),
            diagnostics_logged: AtomicBool::new(false),
        }
    }

    /// Open the Spout receiver sharing wgpu's D3D12 device and command queue.
    fn open_receiver(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sender_name: &str,
    ) -> Result<spout2::dx12::Receiver, String> {
        // Safety: device/queue outlive the receiver; the app owns both for its lifetime.
        unsafe {
            let Some(hal_device) = device.as_hal::<Dx12Api>() else {
                return Err("Spout requires wgpu's D3D12 backend".to_string());
            };
            let Some(hal_queue) = queue.as_hal::<Dx12Api>() else {
                return Err("Spout requires wgpu's D3D12 backend".to_string());
            };
            let device_ptr = hal_device.raw_device().as_raw();
            let mut queue_ptr = hal_queue.as_raw().as_raw();
            spout2::dx12::Receiver::with_device(Some(sender_name), device_ptr, &mut queue_ptr)
                .map_err(|e| e.to_string())
        }
    }

    /// The raw `ID3D12Resource*` backing a wgpu texture, for Spout's receive.
    fn texture_ptr(texture: &wgpu::Texture) -> Result<*mut c_void, &'static str> {
        // Safety: the texture is alive for this call and shares the receiver's device.
        unsafe {
            let Some(hal_texture) = texture.as_hal::<Dx12Api>() else {
                return Err("Spout requires wgpu's D3D12 backend");
            };
            Ok(hal_texture.raw_resource().as_raw())
        }
    }

    /// Map the sender's `DXGI_FORMAT` to a wgpu texture format.
    fn wgpu_format(dxgi: u32) -> Option<wgpu::TextureFormat> {
        Some(match dxgi {
            Dx12Format::B8G8R8A8_UNORM => wgpu::TextureFormat::Bgra8Unorm,
            Dx12Format::R8G8B8A8_UNORM => wgpu::TextureFormat::Rgba8Unorm,
            Dx12Format::R8G8B8A8_UNORM_SRGB => wgpu::TextureFormat::Rgba8UnormSrgb,
            // DXGI_FORMAT_B8G8R8A8_UNORM_SRGB (no constant in spout2::dx12::format).
            91 => wgpu::TextureFormat::Bgra8UnormSrgb,
            Dx12Format::R10G10B10A2_UNORM => wgpu::TextureFormat::Rgb10a2Unorm,
            Dx12Format::R16G16B16A16_FLOAT => wgpu::TextureFormat::Rgba16Float,
            _ => return None,
        })
    }

    fn format_label(dxgi: u32) -> &'static str {
        match Self::wgpu_format(dxgi) {
            Some(wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) => {
                PixelFormat::Bgra8.label()
            }
            Some(_) => PixelFormat::Rgba8.label(),
            None => "unknown",
        }
    }

    /// The last produced frame, if any.
    fn cached(&self) -> Option<Frame> {
        let dims = self.dims.lock().unwrap();
        let bg = self.bg.lock().unwrap();
        bg.as_ref().map(|bg| {
            Frame::Gpu(GpuFrame {
                bg: bg.clone(),
                w: dims.0,
                h: dims.1,
                seq: self.seq.load(Ordering::Relaxed),
                flip_v: false,
            })
        })
    }
}

impl VideoSource for SpoutSource {
    fn latest(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Frame> {
        let mut receiver_guard = self.receiver.lock().unwrap();

        if receiver_guard.is_none() {
            match Self::open_receiver(device, queue, &self.source_ref) {
                Ok(receiver) => {
                    tracing::info!(source=self.source_ref, "Spout receiver opened");
                    *receiver_guard = Some(SpoutReceiver(receiver));
                }
                Err(e) => {
                    // Retried every frame; log the first failure only.
                    if !self.diagnostics_logged.swap(true, Ordering::Relaxed) {
                        tracing::error!(source=self.source_ref, "Spout receiver open failed: {}", e);
                    }
                    return None;
                }
            }
        }
        let receiver = &mut receiver_guard.as_mut().unwrap().0;

        // Sender appeared or changed: drop the old texture/bind group so both rebuild below.
        if receiver.is_updated() {
            *self.texture.lock().unwrap() = None;
            *self.bg.lock().unwrap() = None;
            *self.dims.lock().unwrap() = (0, 0);
        }

        // Deliberately not gated on `is_connected()`: Spout only sets that flag after a
        // receive into an existing texture (the null slot returns first), so the gate deadlocks.
        if self.texture.lock().unwrap().is_none() {
            let (w, h) = receiver.sender_size();
            if w > 0 && h > 0 {
                let Some(format) = Self::wgpu_format(receiver.sender_format()) else {
                    if !self.diagnostics_logged.swap(true, Ordering::Relaxed) {
                        tracing::warn!(source=self.source_ref, "Spout sender uses unsupported DXGI format {}", receiver.sender_format());
                    }
                    return None;
                };
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(self.source_ref.as_str()),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                *self.dims.lock().unwrap() = (w, h);
                *self.texture.lock().unwrap() = Some(texture);
            }
        }

        // Null slot discovers the sender; a texture pointer makes Spout copy into it.
        let mut raw_slot: *mut c_void = std::ptr::null_mut();
        {
            let texture_guard = self.texture.lock().unwrap();
            if let Some(texture) = texture_guard.as_ref() {
                match Self::texture_ptr(texture) {
                    Ok(ptr) => raw_slot = ptr,
                    Err(e) => {
                        tracing::error!(source=self.source_ref, "Error on texture pointer: {}", e);
                        return None;
                    }
                }
            }
        }
        // Safety: slot holds our texture's `ID3D12Resource*` (same device) or null.
        match unsafe { receiver.receive_resource(&mut raw_slot) } {
            Ok(true) => {}
            // Not connected to a sender yet.
            Ok(false) => return self.cached(),
            Err(e) => {
                tracing::debug!(source=self.source_ref, "Spout receive failed: {}", e);
                return self.cached();
            }
        }

        let texture_guard = self.texture.lock().unwrap();
        let Some(texture) = texture_guard.as_ref() else {
            return self.cached();
        };
        let frame_new = receiver.is_frame_new();

        let dims = self.dims.lock().unwrap();
        let mut bg = self.bg.lock().unwrap();
        let mut layout = self.layout.lock().unwrap();
        let mut sampler = self.sampler.lock().unwrap();

        if layout.is_none() {
            let l = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("spout"),
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

        if bg.is_none() {
            let view = texture.create_view(&Default::default());
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
                    width: dims.0 as f32,
                    height: dims.1 as f32,
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
        }

        if frame_new {
            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            let (w, h) = (dims.0, dims.1);
            {
                let mut s = self.stats.lock().unwrap();
                s.record_frame(w, h, Self::format_label(receiver.sender_format()), 0.0);
                s.record_copy_time(0.0);
            }
            return Some(Frame::Gpu(GpuFrame {
                bg: bg.as_ref().unwrap().clone(),
                w,
                h,
                seq,
                flip_v: false,
            }));
        }

        // Return the cached frame even if no new frame arrived.
        Some(Frame::Gpu(GpuFrame {
            bg: bg.as_ref().unwrap().clone(),
            w: dims.0,
            h: dims.1,
            seq: self.seq.load(Ordering::Relaxed),
            flip_v: false,
        }))
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        self.stats.clone()
    }
}
