use std::ffi::c_void;
use std::sync::Arc;

use objc2_io_surface::IOSurfaceRef;
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTextureDescriptor, MTLTextureType,
    MTLTextureUsage,
};
use wgpu::hal::api::Metal as MetalApi;

use crate::sources::{ConvUniform, Frame, GpuFrame, PixelFormat};

/// A retained CVPixelBufferRef. The release function comes from the producer
/// because core-video and the DeckLink shim create the buffer differently.
pub struct CvFrame {
    ptr: *mut c_void,
    iosurface: *mut c_void,
    pub w: u32,
    pub h: u32,
    pub seq: u64,
    pub fmt: PixelFormat,
    release: unsafe extern "C" fn(*mut c_void),
}

unsafe impl Send for CvFrame {}
unsafe impl Sync for CvFrame {}

impl CvFrame {
    pub fn new(
        ptr: *mut c_void,
        iosurface: *mut c_void,
        w: u32,
        h: u32,
        seq: u64,
        fmt: PixelFormat,
        release: unsafe extern "C" fn(*mut c_void),
    ) -> Self {
        return Self {
            ptr,
            iosurface,
            w,
            h,
            seq,
            fmt,
            release,
        };
    }
}

impl Drop for CvFrame {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { (self.release)(self.ptr) };
        }
    }
}

/// Builds `Frame::Gpu` frames from IOSurface-backed CVPixelBuffers.
pub struct CvFramePipeline {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
}

impl CvFramePipeline {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("decklink-cv"),
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
            label: Some("decklink-cv"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("decklink-cv-conv"),
            size: std::mem::size_of::<ConvUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &uniform,
            0,
            bytemuck::cast_slice(&[ConvUniform {
                mode: 0,
                width: 1.0,
                height: 1.0,
                _pad: 0.0,
            }]),
        );
        return Some(Self {
            layout,
            sampler,
            uniform,
        });
    }

    pub fn frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cv: CvFrame,
    ) -> Option<Frame> {
        let (format, tex_w, mode) = match cv.fmt {
            PixelFormat::Bgra8 => (wgpu::TextureFormat::Bgra8Unorm, cv.w, 0u32),
            PixelFormat::Uyvy422 => (
                wgpu::TextureFormat::Rg8Unorm,
                cv.w,
                if cv.h > 576 { 8 } else { 7 },
            ),
            _ => return None,
        };
        let texture = texture_from_iosurface(
            device,
            cv.iosurface,
            tex_w,
            cv.h,
            format,
            wgpu::TextureUsages::TEXTURE_BINDING,
            wgpu::TextureUses::RESOURCE,
        )?;
        let view = texture.create_view(&Default::default());
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&[ConvUniform {
                mode,
                width: cv.w as f32,
                height: cv.h as f32,
                _pad: 0.0,
            }]),
        );
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decklink-cv"),
            layout: &self.layout,
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
                        buffer: &self.uniform,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });
        return Some(Frame::Gpu(GpuFrame {
            bg: Arc::new(bg),
            w: cv.w,
            h: cv.h,
            seq: cv.seq,
            flip_v: false,
        }));
    }
}

/// Wraps an IOSurface as a wgpu texture. `usage`/`hal_usage` differ between a
/// render target (DeckLink output) and a sampled input texture.
pub fn texture_from_iosurface(
    device: &wgpu::Device,
    iosurface: *mut c_void,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    hal_usage: wgpu::TextureUses,
) -> Option<wgpu::Texture> {
    if iosurface.is_null() {
        return None;
    }
    let hal = unsafe { device.as_hal::<MetalApi>() }?;
    let mtl_device = hal.raw_device();
    let mtl_format = match format {
        wgpu::TextureFormat::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
        wgpu::TextureFormat::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
        wgpu::TextureFormat::Rg8Unorm => MTLPixelFormat::RG8Unorm,
        _ => return None,
    };
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            mtl_format,
            width as usize,
            height as usize,
            false,
        )
    };
    let mut mtl_usage = MTLTextureUsage::ShaderRead;
    if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
        mtl_usage |= MTLTextureUsage::RenderTarget;
    }
    desc.setUsage(mtl_usage);
    desc.setStorageMode(MTLStorageMode::Shared);
    let iosurface = unsafe { &*iosurface.cast::<IOSurfaceRef>() };
    let raw = mtl_device.newTextureWithDescriptor_iosurface_plane(&desc, iosurface, 0)?;
    let hal_texture = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            raw,
            format,
            MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
            None,
        )
    };
    let texture_desc = wgpu::TextureDescriptor {
        label: Some("decklink-iosurface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    };
    // SAFETY: the hal texture wraps the IOSurface-backed Metal texture; wgpu
    // owns it from here and the desktop video frame owns the CV buffer.
    let texture = unsafe {
        device.create_texture_from_hal::<MetalApi>(hal_texture, &texture_desc, hal_usage)
    };
    return Some(texture);
}
