use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use multiviewer_decklink::{DecklinkPixelFormat, VideoConnection};
use multiviewer_decklink::{decklink_source_new, decklink_source_free, decklink_source_set_connection, decklink_source_start, decklink_source_stop, decklink_source_poll_frame};
use multiviewer_decklink::{decklink_source_frame_generation, decklink_source_wait_frame};
use multiviewer_decklink::{
    decklink_source_disable_pinned, decklink_source_enable_pinned,
    decklink_source_pinned_available, decklink_source_poll_pinned_frame,
    decklink_source_return_buffer, decklink_source_set_capture_buffers,
};

#[cfg(target_os = "macos")]
use std::ffi::c_void;
#[cfg(target_os = "macos")]
use std::ptr::null_mut;
#[cfg(target_os = "macos")]
use multiviewer_decklink::{
    decklink_cv_buffer_iosurface, decklink_cv_frame_release, decklink_source_is_bgra,
    decklink_source_poll_cv_frame, decklink_source_set_gpu_mode,
    decklink_source_take_cpu_fallback,
};

#[cfg(target_os = "macos")]
use crate::platform::{CvFrame, CvFramePipeline};
use crate::sources::{ConvUniform, CpuFrame, Frame, FramePool, GpuFrame, PixelFormat, SourceRef, SourceStats, VideoSource};

// @docs protocol="DeckLink" direction=input
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DecklinkSourceConfig {
    /// Physical input to capture from. `Unspecified` falls back to whichever connection the device reports first.
    pub connection: VideoConnection,
}

#[derive(Clone, Copy)]
struct PinnedFrame {
    index: usize,
    w: u32,
    h: u32,
    seq: u64,
    fmt: u32,
}

struct PinnedRequest {
    pointers: Vec<*mut u8>,
    sizes: Vec<u32>,
}

unsafe impl Send for PinnedRequest {}

struct PinnedState {
    buffers: Vec<wgpu::Buffer>,
    texture: wgpu::Texture,
    bind_group: Arc<wgpu::BindGroup>,
    dims: (u32, u32),
    fmt: u32,
    bpp: u32,
    tex_w: u32,
    tex_h: u32,
    pending: VecDeque<(usize, wgpu::SubmissionIndex)>,
}

impl PinnedState {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        fmt: u32,
    ) -> Option<(Self, PinnedRequest)> {
        let (texture_format, tex_w, bpp, mode) = match DecklinkPixelFormat::try_from(fmt) {
            Ok(DecklinkPixelFormat::Bgra8) => (wgpu::TextureFormat::Bgra8Unorm, width, 4u32, 0u32),
            Ok(DecklinkPixelFormat::Uyvy422) => (
                wgpu::TextureFormat::Rgba8Unorm,
                width / 2,
                2u32,
                if height > 576 { 2 } else { 1 },
            ),
            _ => return None,
        };
        if width == 0 || height == 0 || (width * bpp) % 256 != 0 {
            return None;
        }
        let tex_h = height;

        const RING: usize = 4;
        let size = (width * height * bpp) as u64;
        let mut buffers = Vec::with_capacity(RING);
        let mut pointers = Vec::with_capacity(RING);
        for _ in 0..RING {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("decklink-pinned"),
                size,
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let mapped = Arc::new(AtomicBool::new(false));
            let map_failed = Arc::new(AtomicBool::new(false));
            buffer.map_async(wgpu::MapMode::Write, .., {
                let mapped = mapped.clone();
                let map_failed = map_failed.clone();
                move |result| {
                    if result.is_ok() {
                        mapped.store(true, Ordering::Release);
                    } else {
                        map_failed.store(true, Ordering::Release);
                    }
                }
            });
            let deadline = Instant::now() + Duration::from_millis(100);
            while !mapped.load(Ordering::Acquire)
                && !map_failed.load(Ordering::Acquire)
                && Instant::now() < deadline
            {
                let _ = device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_millis(10)),
                });
            }
            if !mapped.load(Ordering::Acquire) {
                return None;
            }
            {
                let Ok(mut view) = buffer.slice(..).get_mapped_range_mut() else {
                    return None;
                };
                let mut write = view.slice(..);
                pointers.push(write.as_raw_ptr().as_ptr() as *mut u8);
                std::mem::forget(write);
                std::mem::forget(view);
            }
            buffers.push(buffer);
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("decklink-pinned"),
            size: wgpu::Extent3d {
                width: tex_w,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("decklink-pinned"),
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
            label: Some("decklink-pinned"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("decklink-pinned-conv"),
            size: std::mem::size_of::<ConvUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &uniform,
            0,
            bytemuck::cast_slice(&[ConvUniform {
                mode,
                width: width as f32,
                height: height as f32,
                _pad: 0.0,
            }]),
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decklink-pinned"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
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

        let state = Self {
            buffers,
            texture,
            bind_group: Arc::new(bind_group),
            dims: (width, height),
            fmt,
            bpp,
            tex_w,
            tex_h,
            pending: VecDeque::new(),
        };
        let request = PinnedRequest {
            pointers,
            sizes: vec![size as u32; RING],
        };
        return Some((state, request));
    }

    fn reap(&mut self, device: &wgpu::Device, completed: &Mutex<Vec<usize>>) {
        let mut pending = std::mem::take(&mut self.pending);
        while let Some((index, submission)) = pending.pop_front() {
            let done = device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission.clone()),
                    timeout: Some(Duration::ZERO),
                })
                .is_ok();
            if done {
                completed.lock().unwrap().push(index);
            } else {
                self.pending.push_back((index, submission));
            }
        }
    }
}

struct CaptureShared {
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    pinned_slot: Arc<Mutex<Option<PinnedFrame>>>,
    pinned_request: Arc<Mutex<Option<PinnedRequest>>>,
    return_queue: Arc<Mutex<Vec<usize>>>,
    desired_pinned: Arc<AtomicBool>,
    info: Arc<Mutex<Option<(u32, u32, u32)>>>,
    #[cfg(target_os = "macos")]
    cv_slot: Arc<Mutex<Option<CvFrame>>>,
    #[cfg(target_os = "macos")]
    gpu: Arc<AtomicBool>,
}

pub struct DecklinkSource {
    source_ref: SourceRef,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pinned_slot: Arc<Mutex<Option<PinnedFrame>>>,
    pinned_request: Arc<Mutex<Option<PinnedRequest>>>,
    return_queue: Arc<Mutex<Vec<usize>>>,
    desired_pinned: Arc<AtomicBool>,
    info: Arc<Mutex<Option<(u32, u32, u32)>>>,
    pinned: Mutex<Option<PinnedState>>,
    pinned_attempted: AtomicBool,
    pinned_frame: Mutex<Option<Frame>>,
    #[cfg(target_os = "macos")]
    cv_slot: Arc<Mutex<Option<CvFrame>>>,
    #[cfg(target_os = "macos")]
    gpu: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    pipeline: Mutex<Option<CvFramePipeline>>,
    #[cfg(target_os = "macos")]
    gpu_frame: Mutex<Option<Frame>>,
    #[cfg(target_os = "macos")]
    last_frame: Mutex<Option<Frame>>,
}

impl DecklinkSource {
    pub fn spawn(source_ref: SourceRef, cfg: &DecklinkSourceConfig) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let running = Arc::new(AtomicBool::new(true));
        let pinned_slot = Arc::new(Mutex::new(None));
        let pinned_request = Arc::new(Mutex::new(None));
        let return_queue = Arc::new(Mutex::new(Vec::new()));
        let desired_pinned = Arc::new(AtomicBool::new(false));
        let info = Arc::new(Mutex::new(None));
        #[cfg(target_os = "macos")]
        let cv_slot = Arc::new(Mutex::new(None));
        #[cfg(target_os = "macos")]
        let gpu = Arc::new(AtomicBool::new(true));

        let shared = CaptureShared {
            slot: slot.clone(),
            stats: stats.clone(),
            running: running.clone(),
            pinned_slot: pinned_slot.clone(),
            pinned_request: pinned_request.clone(),
            return_queue: return_queue.clone(),
            desired_pinned: desired_pinned.clone(),
            info: info.clone(),
            #[cfg(target_os = "macos")]
            cv_slot: cv_slot.clone(),
            #[cfg(target_os = "macos")]
            gpu: gpu.clone(),
        };

        let trace_ref = source_ref.clone();
        let connection = cfg.connection;
        let thread = std::thread::Builder::new()
            .name(format!("decklink-in-{source_ref}"))
            .spawn(move || {
                run_capture(trace_ref, connection, shared);
            })
            .expect("spawn decklink source");
        Self {
            source_ref,
            slot,
            stats,
            running,
            thread: Some(thread),
            pinned_slot,
            pinned_request,
            return_queue,
            desired_pinned,
            info,
            pinned: Mutex::new(None),
            pinned_attempted: AtomicBool::new(false),
            pinned_frame: Mutex::new(None),
            #[cfg(target_os = "macos")]
            cv_slot,
            #[cfg(target_os = "macos")]
            gpu,
            #[cfg(target_os = "macos")]
            pipeline: Mutex::new(None),
            #[cfg(target_os = "macos")]
            gpu_frame: Mutex::new(None),
            #[cfg(target_os = "macos")]
            last_frame: Mutex::new(None),
        }
    }

    fn ensure_pinned(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if !cfg!(any(target_os = "windows", target_os = "linux")) {
            return;
        }
        if self.desired_pinned.load(Ordering::Relaxed) || self.pinned.lock().unwrap().is_some() {
            return;
        }
        let Some((w, h, fmt)) = *self.info.lock().unwrap() else { return; };
        if self.pinned_attempted.swap(true, Ordering::Relaxed) {
            return;
        }
        if let Some((state, request)) = PinnedState::new(device, queue, w, h, fmt) {
            *self.pinned.lock().unwrap() = Some(state);
            *self.pinned_request.lock().unwrap() = Some(request);
            self.desired_pinned.store(true, Ordering::Relaxed);
        }
    }

    fn reap_pinned(&self, device: &wgpu::Device) {
        let mut pinned = self.pinned.lock().unwrap();
        if let Some(state) = pinned.as_mut() {
            state.reap(device, &self.return_queue);
        }
    }

    fn copy_pinned(
        &self,
        pending: PinnedFrame,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<Frame> {
        let mut pinned = self.pinned.lock().unwrap();
        let state = pinned.as_mut()?;
        if state.dims != (pending.w, pending.h) || state.fmt != pending.fmt {
            self.desired_pinned.store(false, Ordering::Relaxed);
            return None;
        }
        if pending.index >= state.buffers.len() {
            return None;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("decklink-pinned"),
        });
        // wgpu-core does not reject a mapped buffer as a copy source; the card
        // writes into the mapping while the GPU reads it. Re-check on a wgpu bump.
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &state.buffers[pending.index],
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pending.w * state.bpp),
                    rows_per_image: Some(pending.h),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture: &state.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: state.tex_w,
                height: state.tex_h,
                depth_or_array_layers: 1,
            },
        );
        let submission = queue.submit(Some(encoder.finish()));
        state.pending.push_back((pending.index, submission));
        return Some(Frame::Gpu(GpuFrame {
            bg: state.bind_group.clone(),
            w: pending.w,
            h: pending.h,
            seq: pending.seq,
            flip_v: false,
        }));
    }
}

fn run_capture(source_ref: SourceRef, connection: VideoConnection, shared: CaptureShared) {
    let device_name = source_ref.clone();
    unsafe {
        let display_name_c = match CString::new(device_name) {
            Ok(c) => c,
            Err(_) => {
                tracing::error!(source=source_ref, "DeckLink display name contains null");
                return;
            }
        };
        let src = decklink_source_new(display_name_c.as_ptr());
        if src.is_null() {
            tracing::error!(source=source_ref, "DeckLink source creation failed");
            return;
        }
        if !matches!(connection, VideoConnection::Unspecified) {
            decklink_source_set_connection(src, connection as u32);
        }
        #[cfg(target_os = "macos")]
        decklink_source_set_gpu_mode(src, shared.gpu.load(Ordering::Relaxed));
        if !decklink_source_start(src) {
            tracing::error!(source=source_ref, "DeckLink source start failed");
            decklink_source_free(src);
            return;
        }
        #[cfg(target_os = "macos")]
        let mut shim_gpu = shared.gpu.load(Ordering::Relaxed);

        const MAX_SIZE: usize = 3840 * 2160 * 4;
        let mut pool = FramePool::new();
        let mut last_seq = 0u64;
        let mut last_published = Instant::now();
        let mut no_frames_warned = false;
        let mut shim_pinned = false;
        while shared.running.load(Ordering::Relaxed) {
            if last_published.elapsed() > Duration::from_secs(3) && !no_frames_warned {
                no_frames_warned = true;
                #[cfg(target_os = "macos")]
                tracing::warn!(
                    source = source_ref,
                    gpu = shared.gpu.load(Ordering::Relaxed),
                    bgra = decklink_source_is_bgra(src),
                    "DeckLink produced no frames for 3s; restarting the input"
                );
                #[cfg(not(target_os = "macos"))]
                tracing::warn!(
                    source = source_ref,
                    "DeckLink produced no frames for 3s; restarting the input"
                );
                decklink_source_stop(src);
                if decklink_source_start(src) {
                    last_published = Instant::now();
                    no_frames_warned = false;
                    tracing::info!(source = source_ref, "DeckLink input restarted after no frames");
                } else {
                    tracing::warn!(source = source_ref, "DeckLink input restart failed");
                }
            }
            if cfg!(any(target_os = "windows", target_os = "linux")) {
                let returns = std::mem::take(&mut *shared.return_queue.lock().unwrap());
                for index in returns {
                    decklink_source_return_buffer(src, index as i32);
                }

                let want_pinned = shared.desired_pinned.load(Ordering::Relaxed);
                if want_pinned && !shim_pinned {
                    if let Some(request) = shared.pinned_request.lock().unwrap().take() {
                        let mut pointers = request.pointers;
                        let ok = decklink_source_set_capture_buffers(
                            src,
                            pointers.as_mut_ptr(),
                            request.sizes.as_ptr(),
                            request.sizes.len() as u32,
                        ) && decklink_source_pinned_available(src)
                            && decklink_source_enable_pinned(src);
                        if ok {
                            shim_pinned = true;
                            tracing::info!(source=source_ref, "DeckLink capture uses caller buffers");
                        } else {
                            shared.desired_pinned.store(false, Ordering::Relaxed);
                            tracing::warn!(source=source_ref, "DeckLink caller buffers unavailable; using the CPU path");
                        }
                    }
                } else if !want_pinned && shim_pinned {
                    decklink_source_disable_pinned(src);
                    shim_pinned = false;
                }

                if shim_pinned {
                    let seen = decklink_source_frame_generation(src);
                    let mut index = -1i32;
                    let mut w = 0i32;
                    let mut h = 0i32;
                    let mut seq = 0u64;
                    let mut fmt = 0u32;
                    let mut nominal_fps = 0.0f64;
                    if decklink_source_poll_pinned_frame(
                        src,
                        &mut index,
                        &mut w,
                        &mut h,
                        &mut seq,
                        &mut fmt,
                        &mut nominal_fps,
                    ) {
                        let frame_span = tracing::debug_span!("decklink_frame");
                        let _frame_guard = frame_span.entered();
                        {
                            let mut s = shared.stats.lock().unwrap();
                            let label = if fmt == DecklinkPixelFormat::Bgra8 as u32 {
                                PixelFormat::Bgra8.label()
                            } else {
                                PixelFormat::Uyvy422.label()
                            };
                            s.record_frame(w as u32, h as u32, label, nominal_fps);
                            s.record_copy_time(0.0);
                            s.set_gpu_direct();
                        }
                        let frame = PinnedFrame {
                            index: index as usize,
                            w: w as u32,
                            h: h as u32,
                            seq,
                            fmt,
                        };
                        let old = shared.pinned_slot.lock().unwrap().replace(frame);
                        if let Some(old) = old {
                            decklink_source_return_buffer(src, old.index as i32);
                        }
                        last_published = Instant::now();
                        no_frames_warned = false;
                        continue;
                    }
                    decklink_source_wait_frame(src, seen, 100);
                    continue;
                }
            }

            #[cfg(target_os = "macos")]
            {
                let want = shared.gpu.load(Ordering::Relaxed);
                if want != shim_gpu {
                    decklink_source_set_gpu_mode(src, want);
                    shim_gpu = want;
                }
                if shim_gpu {
                    let seen = decklink_source_frame_generation(src);
                    let mut cv: *mut c_void = null_mut();
                    let mut w = 0;
                    let mut h = 0;
                    let mut seq = 0u64;
                    let mut fmt = 0u32;
                    let mut nominal_fps = 0.0f64;
                    if decklink_source_poll_cv_frame(
                        src,
                        &mut cv,
                        &mut w,
                        &mut h,
                        &mut seq,
                        &mut fmt,
                        &mut nominal_fps,
                    ) {
                        let frame_span = tracing::debug_span!("decklink_frame");
                        let _frame_guard = frame_span.entered();
                        let pixel_format = match DecklinkPixelFormat::try_from(fmt) {
                            Ok(DecklinkPixelFormat::Bgra8) => PixelFormat::Bgra8,
                            Ok(DecklinkPixelFormat::Uyvy422) => PixelFormat::Uyvy422,
                            _ => PixelFormat::Rgba8,
                        };
                        {
                            let mut s = shared.stats.lock().unwrap();
                            s.record_frame(
                                w as u32,
                                h as u32,
                                pixel_format.label(),
                                nominal_fps,
                            );
                            s.record_copy_time(0.0);
                            s.set_gpu_direct();
                        }
                        let iosurface = decklink_cv_buffer_iosurface(cv);
                        let frame = CvFrame::new(
                            cv,
                            iosurface,
                            w as u32,
                            h as u32,
                            seq,
                            pixel_format,
                            decklink_cv_frame_release,
                        );
                        let old = shared.cv_slot.lock().unwrap().replace(frame);
                        drop(old);
                        last_published = Instant::now();
                        no_frames_warned = false;
                        continue;
                    }
                    if !decklink_source_take_cpu_fallback(src) {
                        decklink_source_wait_frame(src, seen, 100);
                        continue;
                    }
                }
            }

            let seen = decklink_source_frame_generation(src);
            let mut buf = pool.take(MAX_SIZE);
            let mut w = 0;
            let mut h = 0;
            let mut seq = 0u64;
            let mut fmt = 0u32;
            let mut nominal_fps = 0.0f64;
            let t0 = Instant::now();
            let got = decklink_source_poll_frame(
                src,
                buf.as_mut_ptr(),
                buf.len(),
                &mut w,
                &mut h,
                &mut seq,
                &mut fmt,
                &mut nominal_fps,
            );
            if got && seq != last_seq {
                let frame_span = tracing::debug_span!("decklink_frame");
                let _frame_guard = frame_span.entered();
                let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;
                let pixel_format = match DecklinkPixelFormat::try_from(fmt) {
                    Ok(DecklinkPixelFormat::Bgra8) => PixelFormat::Bgra8,
                    Ok(DecklinkPixelFormat::Uyvy422) => PixelFormat::Uyvy422,
                    _ => PixelFormat::Rgba8,
                };
                let bpp = match pixel_format {
                    PixelFormat::Uyvy422 => 2,
                    _ => 4,
                };
                let data_size = (w * h * bpp) as usize;
                buf.truncate(data_size);

                {
                    let mut s = shared.stats.lock().unwrap();
                    s.record_frame(
                        w as u32,
                        h as u32,
                        pixel_format.label(),
                        nominal_fps,
                    );
                    s.record_copy_time(copy_ms);
                }
                last_seq = seq;

                let mut guard = shared.slot.lock().unwrap();
                let old = guard.replace(Frame::Cpu(CpuFrame {
                    data: Arc::new(buf),
                    w: w as u32,
                    h: h as u32,
                    fmt: pixel_format,
                    pitch: 0,
                    seq,
                }));
                drop(guard);

                pool.give(old);
                *shared.info.lock().unwrap() = Some((w as u32, h as u32, fmt));
                last_published = Instant::now();
                no_frames_warned = false;
                continue;
            }
            pool.release(buf);
            decklink_source_wait_frame(src, seen, 100);
        }
        decklink_source_stop(src);
        decklink_source_free(src);
    }
}

impl Drop for DecklinkSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take()
            && let Err(e) = t.join()
        {
            tracing::error!(source=self.source_ref, "DeckLink thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for DecklinkSource {
    fn latest(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Frame> {
        #[cfg(target_os = "macos")]
        {
            let macos = (|| -> Option<Frame> {
            if self.gpu.load(Ordering::Relaxed) {
                let pending = self.cv_slot.lock().unwrap().take();
                if let Some(cv) = pending {
                    let mut pipeline = self.pipeline.lock().unwrap();
                    if pipeline.is_none() {
                        *pipeline = CvFramePipeline::new(device, queue);
                    }
                    match pipeline.as_mut() {
                        Some(p) => match p.frame(device, queue, cv) {
                            Some(frame) => {
                                *self.gpu_frame.lock().unwrap() = Some(frame);
                            }
                            None => {
                                tracing::warn!(
                                    source = self.source_ref,
                                    "DeckLink IOSurface import failed; using the CPU path"
                                );
                                self.gpu.store(false, Ordering::Relaxed);
                            }
                        },
                        None => {
                            tracing::warn!(
                                source = self.source_ref,
                                "DeckLink has no Metal device; using the CPU path"
                            );
                            self.gpu.store(false, Ordering::Relaxed);
                        }
                    }
                }
                if let Some(frame) = self.gpu_frame.lock().unwrap().clone() {
                    let cpu = self.slot.lock().unwrap().clone();
                    let chosen = if cpu.as_ref().is_some_and(|c| c.seq() > frame.seq()) {
                        cpu
                    } else {
                        Some(frame)
                    };
                    *self.last_frame.lock().unwrap() = chosen.clone();
                    return chosen;
                }
            }
            let cpu = self.slot.lock().unwrap().clone();
            if cpu.is_some() {
                *self.last_frame.lock().unwrap() = cpu.clone();
                return cpu;
            }
            return self.last_frame.lock().unwrap().clone();
            })();
            if macos.is_some() {
                return macos;
            }
        }
        if cfg!(any(target_os = "windows", target_os = "linux")) {
            self.ensure_pinned(device, queue);
            self.reap_pinned(device);
            if let Some(pending) = self.pinned_slot.lock().unwrap().take() {
                if let Some(frame) = self.copy_pinned(pending, device, queue) {
                    *self.pinned_frame.lock().unwrap() = Some(frame.clone());
                    return Some(frame);
                }
            }
            if self.desired_pinned.load(Ordering::Relaxed) {
                if let Some(frame) = self.pinned_frame.lock().unwrap().clone() {
                    return Some(frame);
                }
            }
        }
        return self.slot.lock().unwrap().clone();
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }
}
