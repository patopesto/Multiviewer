use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_io_surface::IOSurfaceRef;
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureType,
    MTLTextureUsage,
};
use wgpu::hal::api::Metal as MetalApi;

use crate::sources::{ConvUniform, Frame, GpuFrame};

type CVMetalTextureCacheRef = *mut c_void;

unsafe extern "C" {
    fn CVMetalTextureCacheCreate(
        allocator: *const c_void,
        cache_attributes: *const c_void,
        device: *const c_void,
        texture_attributes: *const c_void,
        cache_out: *mut CVMetalTextureCacheRef,
    ) -> i32;
    fn CVMetalTextureCacheCreateTextureFromImage(
        allocator: *const c_void,
        cache: CVMetalTextureCacheRef,
        source_image: *mut c_void,
        texture_attributes: *const c_void,
        pixel_format: usize,
        width: usize,
        height: usize,
        plane_index: usize,
        texture_out: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureGetTexture(image: *mut c_void) -> *mut c_void;
    fn CFRelease(cf: *const c_void);
}

/// A retained CVPixelBufferRef. The release function comes from the producer
/// because core-video and the DeckLink shim create the buffer differently.
pub struct CvFrame {
    ptr: *mut c_void,
    pub w: u32,
    pub h: u32,
    pub seq: u64,
    release: unsafe extern "C" fn(*mut c_void),
}

unsafe impl Send for CvFrame {}
unsafe impl Sync for CvFrame {}

impl CvFrame {
    pub fn new(
        ptr: *mut c_void,
        w: u32,
        h: u32,
        seq: u64,
        release: unsafe extern "C" fn(*mut c_void),
    ) -> Self {
        return Self {
            ptr,
            w,
            h,
            seq,
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

struct CvMetalTexture(*mut c_void);

unsafe impl Send for CvMetalTexture {}
unsafe impl Sync for CvMetalTexture {}

impl Drop for CvMetalTexture {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

struct CvDrop {
    _frame: CvFrame,
    _texture: CvMetalTexture,
}

unsafe impl Send for CvDrop {}
unsafe impl Sync for CvDrop {}

struct CvImporter {
    cache: CVMetalTextureCacheRef,
}

// CVMetalTextureCache is thread-safe; the pipeline is only used behind a Mutex.
unsafe impl Send for CvImporter {}
unsafe impl Sync for CvImporter {}

impl CvImporter {
    fn new(device: &wgpu::Device) -> Option<Self> {
        let hal = unsafe { device.as_hal::<MetalApi>() }?;
        let mtl_device = hal.raw_device();
        let mut cache: CVMetalTextureCacheRef = null_mut();
        let status = unsafe {
            CVMetalTextureCacheCreate(
                null(),
                null(),
                &**mtl_device as *const ProtocolObject<dyn MTLDevice> as *const c_void,
                null(),
                &mut cache,
            )
        };
        if status != 0 || cache.is_null() {
            tracing::warn!(
                "CVMetalTextureCacheCreate failed: status {} cache_null {}",
                status,
                cache.is_null()
            );
            return None;
        }
        return Some(Self { cache });
    }

    fn texture(&self, frame: CvFrame, device: &wgpu::Device) -> Option<wgpu::Texture> {
        let (w, h) = (frame.w, frame.h);
        let mut cv_texture: *mut c_void = null_mut();
        let status = unsafe {
            CVMetalTextureCacheCreateTextureFromImage(
                null(),
                self.cache,
                frame.ptr,
                null(),
                MTLPixelFormat::BGRA8Unorm.0,
                w as usize,
                h as usize,
                0,
                &mut cv_texture,
            )
        };
        if status != 0 || cv_texture.is_null() {
            tracing::warn!(
                "CVMetalTextureCacheCreateTextureFromImage failed: status {} texture_null {} ({}x{})",
                status,
                cv_texture.is_null(),
                w,
                h
            );
            return None;
        }
        let mtl_ptr = unsafe { CVMetalTextureGetTexture(cv_texture) };
        if mtl_ptr.is_null() {
            tracing::warn!("CVMetalTextureGetTexture returned null ({}x{})", w, h);
            unsafe { CFRelease(cv_texture) };
            return None;
        }
        let retained = unsafe {
            Retained::<ProtocolObject<dyn MTLTexture>>::retain(
                mtl_ptr.cast::<ProtocolObject<dyn MTLTexture>>(),
            )
        };
        let Some(retained) = retained else {
            tracing::warn!("could not retain the CV-backed Metal texture ({}x{})", w, h);
            return None;
        };
        let drop_state = CvDrop {
            _frame: frame,
            _texture: CvMetalTexture(cv_texture),
        };
        let hal_texture = unsafe {
            wgpu::hal::metal::Device::texture_from_raw(
                retained,
                wgpu::TextureFormat::Bgra8Unorm,
                MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent {
                    width: w,
                    height: h,
                    depth: 1,
                },
                Some(Box::new(move || drop(drop_state))),
            )
        };
        let desc = wgpu::TextureDescriptor {
            label: Some("decklink-cv"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        // SAFETY: the hal texture wraps the IOSurface-backed Metal texture
        // created above and is owned by wgpu from here on; the drop callback
        // releases the CV buffer and CVMetalTexture once wgpu is done with it.
        let texture = unsafe {
            device.create_texture_from_hal::<MetalApi>(
                hal_texture,
                &desc,
                wgpu::TextureUses::RESOURCE,
            )
        };
        return Some(texture);
    }
}

impl Drop for CvImporter {
    fn drop(&mut self) {
        if !self.cache.is_null() {
            unsafe { CFRelease(self.cache) };
        }
    }
}

/// Builds `Frame::Gpu` frames from IOSurface-backed CVPixelBuffers.
pub struct CvFramePipeline {
    importer: CvImporter,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    dims: (u32, u32),
}

impl CvFramePipeline {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        let importer = CvImporter::new(device)?;
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
            importer,
            layout,
            sampler,
            uniform,
            dims: (0, 0),
        });
    }

    pub fn frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cv: CvFrame,
    ) -> Option<Frame> {
        let (w, h, seq) = (cv.w, cv.h, cv.seq);
        let texture = self.importer.texture(cv, device)?;
        let view = texture.create_view(&Default::default());
        if self.dims != (w, h) {
            self.dims = (w, h);
            queue.write_buffer(
                &self.uniform,
                0,
                bytemuck::cast_slice(&[ConvUniform {
                    mode: 0,
                    width: w as f32,
                    height: h as f32,
                    _pad: 0.0,
                }]),
            );
        }
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
            w,
            h,
            seq,
            flip_v: false,
        }));
    }
}

/// Wraps an IOSurface as a wgpu render target.
pub fn texture_from_iosurface(
    device: &wgpu::Device,
    iosurface: *mut c_void,
    width: u32,
    height: u32,
) -> Option<wgpu::Texture> {
    if iosurface.is_null() {
        return None;
    }
    let hal = unsafe { device.as_hal::<MetalApi>() }?;
    let mtl_device = hal.raw_device();
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::BGRA8Unorm,
            width as usize,
            height as usize,
            false,
        )
    };
    desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    desc.setStorageMode(MTLStorageMode::Shared);
    let iosurface = unsafe { &*iosurface.cast::<IOSurfaceRef>() };
    let raw = mtl_device.newTextureWithDescriptor_iosurface_plane(&desc, iosurface, 0)?;
    let hal_texture = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            raw,
            wgpu::TextureFormat::Bgra8Unorm,
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
        label: Some("decklink-output-cv"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    };
    // SAFETY: the hal texture wraps the IOSurface-backed Metal texture; wgpu
    // owns it from here and the desktop video frame owns the CV buffer.
    let texture = unsafe {
        device.create_texture_from_hal::<MetalApi>(
            hal_texture,
            &texture_desc,
            wgpu::TextureUses::COLOR_TARGET,
        )
    };
    return Some(texture);
}
