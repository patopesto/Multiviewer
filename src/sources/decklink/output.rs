use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

use multiviewer_decklink::{
    decklink_output_free, decklink_output_new, decklink_output_present_frame,
    decklink_output_completion_available, decklink_output_start, decklink_output_stop,
    decklink_output_take_completed, DecklinkOutputHandle, DisplayMode,
};

#[cfg(target_os = "macos")]
use std::ffi::c_void;

#[cfg(target_os = "macos")]
use multiviewer_decklink::{
    decklink_cv_buffer_iosurface, decklink_cv_buffer_new, decklink_cv_buffer_release,
    decklink_output_present_cv,
};

#[cfg(target_os = "macos")]
use crate::platform::texture_from_iosurface;

use super::super::{OutputStats, Protocol};
use super::super::output::{OutputId, OutputLiveness, ReadbackPool, VideoOutput};

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

/// A device another process owns fails to enable; retry slowly so we recover
/// when it is released instead of fighting it for the device.
const START_RETRY: Duration = Duration::from_secs(10);

/// How long `Drop` waits for the worker to release the device before detaching.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(250);

/// Longer grace used when a mode/device restart replaces the output: the old
/// worker must release the device before the new one opens it.
const RESTART_GRACE: Duration = Duration::from_secs(2);

/// How often the worker wakes without a frame to reclaim completion tokens.
/// The CV ring and the pinned readback pool can only be refilled after those
/// tokens reach the main thread, so a blocking recv deadlocks once every slot
/// is in flight.
const DRAIN_INTERVAL: Duration = Duration::from_millis(5);

/// Warn only after the CV ring has been full for a sustained run of presents.
#[cfg(target_os = "macos")]
const CV_STARVE_WARN_FRAMES: u32 = 60;

/// How many borrowed frames the device may hold before the worker drops new
/// ones. The shim returns a token when a frame is released, so this is only a
/// safety valve for a stalled device.
const MAX_IN_FLIGHT: usize = 8;

/// Tokens at or above this bit are IOSurface indices, not borrowed-frame ids.
const CV_TOKEN_BIT: u64 = 1 << 63;
const PINNED_TOKEN_BIT: u64 = 1 << 62;

#[cfg(target_os = "macos")]
const CV_SURFACES: usize = 3;

enum OutputFrame {
    Cpu(Vec<u8>),
    Pinned {
        ptr: PinnedPtr,
        index: usize,
        width: u32,
        height: u32,
    },
    #[cfg(target_os = "macos")]
    Cv { cv: CvPtr, index: usize },
}

#[derive(Clone, Copy)]
struct PinnedPtr(*mut u8);

unsafe impl Send for PinnedPtr {}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct CvPtr(*mut c_void);

#[cfg(target_os = "macos")]
unsafe impl Send for CvPtr {}

// A surface may only be handed to the card in `Ready`, after the scale
// submission has completed.
#[cfg(target_os = "macos")]
enum CvState {
    Free,
    Rendering(wgpu::SubmissionIndex),
    Ready,
    Queued,
}

#[cfg(target_os = "macos")]
struct CvSurface {
    cv: CvPtr,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    state: CvState,
}

#[cfg(target_os = "macos")]
impl Drop for CvSurface {
    fn drop(&mut self) {
        unsafe { decklink_cv_buffer_release(self.cv.0) };
    }
}

#[cfg(target_os = "macos")]
impl CvSurface {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Option<Self> {
        let cv = unsafe { decklink_cv_buffer_new(width as i32, height as i32) };
        if cv.is_null() {
            return None;
        }
        let iosurface = unsafe { decklink_cv_buffer_iosurface(cv) };
        let Some(texture) = texture_from_iosurface(
            device,
            iosurface,
            width,
            height,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            wgpu::TextureUses::COLOR_TARGET,
        ) else {
            unsafe { decklink_cv_buffer_release(cv) };
            return None;
        };
        let view = texture.create_view(&Default::default());
        return Some(Self {
            cv: CvPtr(cv),
            _texture: texture,
            view,
            state: CvState::Free,
        });
    }
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct CvRing {
    surfaces: Vec<CvSurface>,
}

#[cfg(target_os = "macos")]
impl CvRing {
    fn ensure(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        if !self.surfaces.is_empty() {
            return true;
        }
        for _ in 0..CV_SURFACES {
            let Some(surface) = CvSurface::new(device, width, height) else {
                self.surfaces.clear();
                return false;
            };
            self.surfaces.push(surface);
        }
        return true;
    }

    fn acquire(&mut self) -> Option<usize> {
        return self
            .surfaces
            .iter()
            .position(|s| matches!(s.state, CvState::Free));
    }

    fn mark_rendering(&mut self, index: usize, submission: wgpu::SubmissionIndex) {
        if let Some(surface) = self.surfaces.get_mut(index) {
            surface.state = CvState::Rendering(submission);
        }
    }

    fn poll_ready(&mut self, device: &wgpu::Device) {
        for surface in &mut self.surfaces {
            let CvState::Rendering(submission) = &surface.state else {
                continue;
            };
            if device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission.clone()),
                    timeout: Some(Duration::ZERO),
                })
                .is_ok()
            {
                surface.state = CvState::Ready;
            }
        }
    }

    fn ready(&self) -> Option<usize> {
        return self
            .surfaces
            .iter()
            .position(|s| matches!(s.state, CvState::Ready));
    }

    fn mark_queued(&mut self, index: usize) {
        if let Some(surface) = self.surfaces.get_mut(index) {
            surface.state = CvState::Queued;
        }
    }

    fn release(&mut self, index: usize) {
        if let Some(surface) = self.surfaces.get_mut(index) {
            surface.state = CvState::Free;
        }
    }

    fn view(&self, index: usize) -> &wgpu::TextureView {
        return &self.surfaces[index].view;
    }

    fn cv(&self, index: usize) -> CvPtr {
        return self.surfaces[index].cv;
    }
}

/// Reclaim every frame the device has released. The token comes back when the
/// worker's buffer is safe to reuse or drop.
fn drain_completed(
    output: *mut DecklinkOutputHandle,
    in_flight: &mut VecDeque<(u64, Vec<u8>)>,
    cv_completed: Option<&Arc<Mutex<Vec<usize>>>>,
    pinned_completed: Option<&Arc<Mutex<Vec<usize>>>>,
) {
    loop {
        let mut token = 0u64;
        // SAFETY: `output` outlives the worker loop that calls this.
        let got = unsafe { decklink_output_take_completed(output, &mut token) };
        if !got {
            return;
        }
        match classify_token(token) {
            TokenKind::Cv(index) => {
                if let Some(completed) = cv_completed {
                    completed.lock().unwrap().push(index);
                }
            }
            TokenKind::Pinned(index) => {
                if let Some(completed) = pinned_completed {
                    completed.lock().unwrap().push(index);
                }
            }
            TokenKind::Vec => {
                if let Some(index) = in_flight.iter().position(|(queued, _)| *queued == token) {
                    in_flight.remove(index);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenKind {
    Cv(usize),
    Pinned(usize),
    Vec,
}

fn classify_token(token: u64) -> TokenKind {
    if token & CV_TOKEN_BIT != 0 {
        return TokenKind::Cv((token & !CV_TOKEN_BIT) as usize);
    }
    if token & PINNED_TOKEN_BIT != 0 {
        return TokenKind::Pinned((token & !PINNED_TOKEN_BIT) as usize);
    }
    return TokenKind::Vec;
}

// @docs protocol="DeckLink" direction=output
#[derive(Serialize, Deserialize, Clone)]
pub struct DecklinkOutputConfig {
    // @docs label="Decklink Device"
    /// Playback device. Empty means no device is selected; the output stays closed.
    pub device_name: String,
    // @docs label="Decklink Mode"
    /// Output video mode: resolution, frame rate and scan. Only modes the device supports are offered.
    pub display_mode: DisplayMode,
    // @docs hidden
    /// Width of the frame sent to the device, in pixels.
    pub width: u32,
    // @docs hidden
    /// Height of the frame sent to the device, in pixels.
    pub height: u32,
    // @docs hidden
    /// Frame rate sent to the device.
    pub fps: f64,
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
    name: String,
    config: DecklinkOutputConfig,
    enabled: AtomicBool,
    stats: Arc<Mutex<OutputStats>>,
    frame_tx: Option<mpsc::SyncSender<OutputFrame>>,
    pending: Arc<AtomicBool>,
    liveness: OutputLiveness,
    readback: Mutex<ReadbackPool>,
    thread: Option<thread::JoinHandle<()>>,
    scale_bind_layout: OnceLock<wgpu::BindGroupLayout>,
    scale_sampler: OnceLock<wgpu::Sampler>,
    scale_pipeline: OnceLock<wgpu::RenderPipeline>,
    scale_vb: OnceLock<wgpu::Buffer>,
    scale_ib: OnceLock<wgpu::Buffer>,
    scaled_texture: Mutex<Option<wgpu::Texture>>,
    scaled_view: Mutex<Option<wgpu::TextureView>>,
    scaled_bind_group: Mutex<Option<wgpu::BindGroup>>,
    scale_source_dims: Mutex<(u32, u32)>,
    #[cfg(target_os = "macos")]
    cv_ring: Mutex<CvRing>,
    #[cfg(target_os = "macos")]
    cv_completed: Arc<Mutex<Vec<usize>>>,
    #[cfg(target_os = "macos")]
    cv_failed: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    use_cv: AtomicBool,
    #[cfg(target_os = "macos")]
    cv_starved: AtomicU32,
    pinned_completed: Arc<Mutex<Vec<usize>>>,
    use_pinned: AtomicBool,
    completion_ok: Arc<AtomicBool>,
}

impl DecklinkOutput {
    pub fn new(id: OutputId, name: String, config: DecklinkOutputConfig, enabled: bool) -> Self {
        let (frame_tx, frame_rx) = mpsc::sync_channel::<OutputFrame>(2);
        let stats = Arc::new(Mutex::new(OutputStats::default()));
        if config.device_name.is_empty() {
            stats.lock().unwrap().error = Some("Select a DeckLink output device".to_string());
        }
        let stats_clone = Arc::clone(&stats);
        let pending = Arc::new(AtomicBool::new(false));
        let pending_t = Arc::clone(&pending);
        let liveness = OutputLiveness::default();
        let worker_flag = liveness.exit_flag();
        #[cfg(target_os = "macos")]
        let cv_completed = Arc::new(Mutex::new(Vec::new()));
        #[cfg(target_os = "macos")]
        let cv_failed = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "macos")]
        let cv_completed_worker = cv_completed.clone();
        #[cfg(target_os = "macos")]
        let cv_failed_worker = cv_failed.clone();
        let pinned_completed = Arc::new(Mutex::new(Vec::new()));
        let pinned_completed_worker = pinned_completed.clone();
        let completion_ok = Arc::new(AtomicBool::new(true));
        let completion_ok_worker = completion_ok.clone();

        let thread_name = name.clone();
        let thread_config = config.clone();
        let thread_fps = config.fps;
        let thread = thread::Builder::new()
            .name(format!("decklink-out-{id}"))
            .spawn(move || {
                let _alive_guard = worker_flag;
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
                let mut next_frame = Instant::now();

                let mut started = false;
                let mut start_logged = false;
                let mut start_failures = 0u32;
                let mut in_flight: VecDeque<(u64, Vec<u8>)> = VecDeque::new();
                let mut next_token = 0u64;
                #[cfg(target_os = "macos")]
                let cv_completed_arg = Some(&cv_completed_worker);
                #[cfg(not(target_os = "macos"))]
                let cv_completed_arg: Option<&Arc<Mutex<Vec<usize>>>> = None;
                loop {
                    let frame = match frame_rx.recv_timeout(DRAIN_INTERVAL) {
                        Ok(frame) => {
                            pending_t.store(false, Ordering::Release);
                            Some(frame)
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    drain_completed(output, &mut in_flight, cv_completed_arg, Some(&pinned_completed_worker));
                    let Some(frame) = frame else { continue; };

                    if !started {
                        if unsafe { decklink_output_start(output, mode_id) } {
                            started = true;
                            start_logged = false;
                            start_failures = 0;
                            stats_clone.lock().unwrap().error = None;
                            completion_ok_worker.store(
                                unsafe { decklink_output_completion_available(output) },
                                Ordering::Relaxed,
                            );
                            tracing::info!(output=thread_name, "DeckLink output started on {}", mode_id);
                        } else {
                            {
                                let mut s = stats_clone.lock().unwrap();
                                s.error = Some("device busy or unavailable".to_string());
                                s.frames_dropped += 1;
                            }
                            start_failures = start_failures.saturating_add(1);
                            if !start_logged {
                                tracing::error!(
                                    output=thread_name,
                                    "DeckLink output start failed; retrying"
                                );
                                start_logged = true;
                            }
                            match &frame {
                                OutputFrame::Pinned { index, .. } => {
                                    pinned_completed_worker.lock().unwrap().push(*index);
                                }
                                #[cfg(target_os = "macos")]
                                OutputFrame::Cv { index, .. } => {
                                    cv_completed_arg
                                        .expect("macOS completions")
                                        .lock()
                                        .unwrap()
                                        .push(*index);
                                }
                                OutputFrame::Cpu(_) => {}
                            }
                            let retry = if start_failures <= 5 {
                                Duration::from_millis(100 * u64::from(start_failures))
                            } else {
                                START_RETRY
                            };
                            thread::sleep(retry);
                            continue;
                        }
                    }

                    {
                        let pacing_span = tracing::debug_span!("decklink_pacing");
                        let _pacing_guard = pacing_span.entered();
                        let now = Instant::now();
                        if now < next_frame {
                            std::thread::sleep(next_frame - now);
                        }
                        // Fixed grid: advance from the deadline, not the actual
                        // wake, so `thread::sleep` overshoot does not pile up
                        // into every frame period (60 -> ~52).
                        let now = Instant::now();
                        next_frame = if now >= next_frame + frame_interval {
                            now + frame_interval
                        } else {
                            next_frame + frame_interval
                        };
                    }

                    match frame {
                        OutputFrame::Cpu(buffer) => {
                            if buffer.len() < 16 {
                                continue;
                            }
                            let frame_w = u32::from_le_bytes([
                                buffer[0], buffer[1], buffer[2], buffer[3],
                            ]);
                            let frame_h = u32::from_le_bytes([
                                buffer[4], buffer[5], buffer[6], buffer[7],
                            ]);
                            let expected = (frame_w * frame_h * 4) as usize;
                            if buffer.len() < expected + 16 {
                                continue;
                            }

                            if in_flight.len() >= MAX_IN_FLIGHT {
                                // The device is not releasing frames; drop this one
                                // rather than reuse a buffer it may still read.
                                stats_clone.lock().unwrap().frames_dropped += 1;
                                continue;
                            }
                            let token = next_token;
                            next_token = next_token.wrapping_add(1);
                            let send_span = tracing::debug_span!("decklink_send");
                            let _send_guard = send_span.entered();
                            let start = Instant::now();
                            let mut token_queued = false;
                            let ok = unsafe {
                                decklink_output_present_frame(
                                    output,
                                    buffer.as_ptr().add(16),
                                    frame_w as i32,
                                    frame_h as i32,
                                    (frame_w * 4) as i32,
                                    token,
                                    &mut token_queued,
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
                            if ok {
                                in_flight.push_back((token, buffer));
                            }
                        }
                        OutputFrame::Pinned {
                            ptr,
                            index,
                            width,
                            height,
                        } => {
                            if !completion_ok_worker.load(Ordering::Relaxed) {
                                pinned_completed_worker.lock().unwrap().push(index);
                                stats_clone.lock().unwrap().frames_dropped += 1;
                                continue;
                            }
                            let row_bytes = ((width * 4 + 255) & !255) as i32;
                            let token = PINNED_TOKEN_BIT | index as u64;
                            let send_span = tracing::debug_span!("decklink_send");
                            let _send_guard = send_span.entered();
                            let start = Instant::now();
                            let mut token_queued = false;
                            let ok = unsafe {
                                decklink_output_present_frame(
                                    output,
                                    ptr.0,
                                    width as i32,
                                    height as i32,
                                    row_bytes,
                                    token,
                                    &mut token_queued,
                                )
                            };

                            {
                                let mut s = stats_clone.lock().unwrap();
                                s.width = width;
                                s.height = height;
                                s.send_time_ms = start.elapsed().as_secs_f32() * 1000.0;
                                if ok {
                                    s.record_sent();
                                } else {
                                    s.frames_dropped += 1;
                                }
                            }
                            if !ok && !token_queued {
                                pinned_completed_worker.lock().unwrap().push(index);
                            }
                        }
                        #[cfg(target_os = "macos")]
                        OutputFrame::Cv { cv, index } => {
                            let token = CV_TOKEN_BIT | index as u64;
                            let send_span = tracing::debug_span!("decklink_send");
                            let _send_guard = send_span.entered();
                            let start = Instant::now();
                            let ok = unsafe {
                                decklink_output_present_cv(output, cv.0, token)
                            };

                            {
                                let mut s = stats_clone.lock().unwrap();
                                s.width = thread_config.width;
                                s.height = thread_config.height;
                                s.send_time_ms = start.elapsed().as_secs_f32() * 1000.0;
                                if ok {
                                    s.record_sent();
                                } else {
                                    s.frames_dropped += 1;
                                }
                            }
                            if !ok {
                                cv_failed_worker.store(true, Ordering::Relaxed);
                                cv_completed_arg
                                    .expect("macOS completions")
                                    .lock()
                                    .unwrap()
                                    .push(index);
                            }
                        }
                    }
                }

                if started {
                    unsafe { decklink_output_stop(output) };
                }
                // Stop cancels scheduled frames; releasing them returns their
                // tokens. Wait briefly, then leak whatever the device still
                // holds rather than free memory it may still be reading.
                let deadline = Instant::now() + SHUTDOWN_GRACE;
                while !in_flight.is_empty() && Instant::now() < deadline {
                    drain_completed(output, &mut in_flight, cv_completed_arg, Some(&pinned_completed_worker));
                    if in_flight.is_empty() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                if !in_flight.is_empty() {
                    tracing::warn!(
                        output=thread_name,
                        "DeckLink output still holds {} frames at shutdown; leaking their buffers",
                        in_flight.len()
                    );
                    for (_, frame) in in_flight.drain(..) {
                        std::mem::forget(frame);
                    }
                }
                unsafe { decklink_output_free(output) };
            })
            .expect("spawn decklink output thread");

        Self {
            name,
            config,
            enabled: AtomicBool::new(enabled),
            stats,
            frame_tx: Some(frame_tx),
            pending,
            liveness,
            readback: Mutex::new(ReadbackPool::new()),
            thread: Some(thread),
            scale_bind_layout: OnceLock::new(),
            scale_sampler: OnceLock::new(),
            scale_pipeline: OnceLock::new(),
            scale_vb: OnceLock::new(),
            scale_ib: OnceLock::new(),
            scaled_texture: Mutex::new(None),
            scaled_view: Mutex::new(None),
            scaled_bind_group: Mutex::new(None),
            scale_source_dims: Mutex::new((0, 0)),
            #[cfg(target_os = "macos")]
            cv_ring: Mutex::new(CvRing::default()),
            #[cfg(target_os = "macos")]
            cv_completed,
            #[cfg(target_os = "macos")]
            cv_failed,
            #[cfg(target_os = "macos")]
            use_cv: AtomicBool::new(true),
            #[cfg(target_os = "macos")]
            cv_starved: AtomicU32::new(0),
            pinned_completed,
            use_pinned: AtomicBool::new(cfg!(any(
                target_os = "windows",
                target_os = "linux"
            ))),
            completion_ok,
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
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ScaleVert>() as u64,
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

    fn ensure_scale_bind_group(
        &self,
        device: &wgpu::Device,
        src_w: u32,
        src_h: u32,
        texture: &wgpu::Texture,
    ) {
        let mut source_dims = self.scale_source_dims.lock().unwrap();
        if *source_dims == (src_w, src_h) {
            return;
        }
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
        *self.scaled_bind_group.lock().unwrap() = Some(bind_group);
        *source_dims = (src_w, src_h);
    }

    fn ensure_scaled_texture(
        &self,
        device: &wgpu::Device,
        out_w: u32,
        out_h: u32,
        src_w: u32,
        src_h: u32,
        texture: &wgpu::Texture,
    ) {
        let mut scaled_texture = self.scaled_texture.lock().unwrap();
        let recreate_texture = scaled_texture
            .as_ref()
            .map(|t| t.width() != out_w || t.height() != out_h)
            .unwrap_or(true);

        if recreate_texture {
            let new_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("decklink-scaled"),
                size: wgpu::Extent3d {
                    width: out_w,
                    height: out_h,
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

            *scaled_texture = Some(new_texture);
            *self.scaled_view.lock().unwrap() = Some(view);
        }

        self.ensure_scale_bind_group(device, src_w, src_h, texture);
    }

    fn render_scale(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        out_w: u32,
        out_h: u32,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Option<wgpu::SubmissionIndex> {
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

        let bind_group = self.scaled_bind_group.lock().unwrap();
        let Some(ref bind_group) = *bind_group else { return None };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("decklink-scale"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("decklink-scale"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
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
        return Some(queue.submit(Some(encoder.finish())));
    }

    #[allow(dead_code)]
    pub fn config(&self) -> &DecklinkOutputConfig {
        &self.config
    }

    #[cfg(target_os = "macos")]
    fn present_cv(
        &self,
        texture: &wgpu::Texture,
        src_w: u32,
        src_h: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<bool> {
        let out_w = self.config.width;
        let out_h = self.config.height;

        {
            let completed: Vec<usize> = std::mem::take(&mut *self.cv_completed.lock().unwrap());
            if !completed.is_empty() {
                let mut ring = self.cv_ring.lock().unwrap();
                for index in completed {
                    ring.release(index);
                }
            }
        }
        if self.cv_failed.load(Ordering::Relaxed) {
            return None;
        }

        let mut ring = self.cv_ring.lock().unwrap();
        if !ring.ensure(device, out_w, out_h) {
            return None;
        }
        ring.poll_ready(device);

        if let Some(index) = ring.ready() {
            self.cv_starved.store(0, Ordering::Relaxed);
            let cv = ring.cv(index);
            drop(ring);
            let Some(tx) = self.frame_tx.as_ref() else {
                self.cv_ring.lock().unwrap().release(index);
                return Some(false);
            };
            self.pending.store(true, Ordering::Release);
            self.liveness.mark_sent();
            if tx.try_send(OutputFrame::Cv { cv, index }).is_err() {
                self.pending.store(false, Ordering::Release);
                self.liveness.clear_sent();
                return Some(false);
            }
            self.cv_ring.lock().unwrap().mark_queued(index);
            return Some(true);
        }

        let Some(index) = ring.acquire() else {
            let starved = self.cv_starved.fetch_add(1, Ordering::Relaxed) + 1;
            if starved == CV_STARVE_WARN_FRAMES {
                tracing::warn!(
                    output = self.name,
                    "DeckLink CV ring exhausted for {} presents; waiting for completion tokens",
                    CV_STARVE_WARN_FRAMES
                );
            }
            return Some(false);
        };
        self.cv_starved.store(0, Ordering::Relaxed);

        self.ensure_scale_resources(device, queue);
        self.ensure_scale_bind_group(device, src_w, src_h, texture);
        let submission = {
            let scale_span = tracing::debug_span!("scale");
            let _scale_guard = scale_span.entered();
            self.render_scale(device, queue, ring.view(index), out_w, out_h, src_w, src_h)
        };
        let Some(submission) = submission else {
            ring.release(index);
            return Some(false);
        };
        ring.mark_rendering(index, submission);
        return Some(true);
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
    ) -> bool {
        {
            let completed: Vec<usize> = std::mem::take(&mut *self.pinned_completed.lock().unwrap());
            if !completed.is_empty() {
                let mut readback = self.readback.lock().unwrap();
                for index in completed {
                    readback.unmap_and_release(index);
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            let completed: Vec<usize> = std::mem::take(&mut *self.cv_completed.lock().unwrap());
            if !completed.is_empty() {
                let mut ring = self.cv_ring.lock().unwrap();
                for index in completed {
                    ring.release(index);
                }
            }
        }

        if !self.enabled.load(Ordering::Relaxed) {
            return false;
        }
        if self.config.device_name.is_empty() {
            return false;
        }
        if self.busy() {
            self.liveness.check_stall(&self.name);
            return false;
        }

        let out_w = self.config.width;
        let out_h = self.config.height;
        if out_w == 0 || out_h == 0 {
            return false;
        }

        #[cfg(target_os = "macos")]
        if self.use_cv.load(Ordering::Relaxed) {
            if let Some(result) = self.present_cv(texture, width, height, device, queue) {
                return result;
            }
            self.use_cv.store(false, Ordering::Relaxed);
        }

        let bytes_per_row = out_w * 4;
        let aligned_bytes_per_row = (bytes_per_row + 255) & !255;
        let buffer_size = (aligned_bytes_per_row * out_h) as u64 + 16;
        let mut readback = self.readback.lock().unwrap();
        readback.ensure(device, "decklink-output-readback", buffer_size);
        let Some(slot) = readback.free() else {
            return false;
        };

        {
            let scale_span = tracing::debug_span!("scale");
            let _scale_guard = scale_span.entered();
            self.ensure_scale_resources(device, queue);
            self.ensure_scaled_texture(device, out_w, out_h, width, height, texture);
            if let Some(view) = self.scaled_view.lock().unwrap().as_ref() {
                let _ = self.render_scale(device, queue, view, out_w, out_h, width, height);
            }
        }

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
                    buffer: readback.buffer(slot),
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

        readback.map(slot, out_w, out_h);
        return true;
    }

    fn finish(&self) {
        let mut readback = self.readback.lock().unwrap();
        let Some(slot) = readback.take_ready() else {
            return;
        };

        let (out_w, out_h) = readback.dims(slot);
        let row = (out_w * 4) as usize;
        let aligned = (row + 255) & !255;

        if self.use_pinned.load(Ordering::Relaxed) && self.completion_ok.load(Ordering::Relaxed) {
            let readback_buffer = readback.buffer(slot);
            let ptr = match readback_buffer.slice(..).get_mapped_range() {
                Ok(view) => {
                    let ptr = unsafe { view.as_ptr().add(16) } as *mut u8;
                    std::mem::forget(view);
                    ptr
                }
                Err(e) => {
                    tracing::error!(output = self.name, "DeckLink output mapped range unavailable: {e:?}");
                    readback_buffer.unmap();
                    readback.release(slot);
                    return;
                }
            };

            let Some(tx) = self.frame_tx.as_ref() else {
                readback_buffer.unmap();
                readback.release(slot);
                return;
            };
            self.pending.store(true, Ordering::Release);
            self.liveness.mark_sent();
            let message = OutputFrame::Pinned {
                ptr: PinnedPtr(ptr),
                index: slot,
                width: out_w,
                height: out_h,
            };
            if tx.try_send(message).is_err() {
                self.pending.store(false, Ordering::Release);
                self.liveness.clear_sent();
                readback_buffer.unmap();
                readback.release(slot);
                self.stats.lock().unwrap().frames_dropped += 1;
            }
            return;
        }

        let mut buffer = Vec::with_capacity(16 + row * out_h as usize);
        let copied = {
            let readback_buffer = readback.buffer(slot);
            match readback_buffer.slice(..).get_mapped_range() {
                Ok(view) => {
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
                    true
                }
                Err(e) => {
                    tracing::error!(output = self.name, "DeckLink output mapped range unavailable: {e:?}");
                    readback_buffer.unmap();
                    false
                }
            }
        };
        readback.release(slot);
        if !copied {
            return;
        }

        buffer[0..4].copy_from_slice(&out_w.to_le_bytes());
        buffer[4..8].copy_from_slice(&out_h.to_le_bytes());
        let mode_id: u32 = self.config.display_mode.into();
        buffer[8..12].copy_from_slice(&mode_id.to_le_bytes());

        let Some(tx) = self.frame_tx.as_ref() else {
            return;
        };
        // Publish in-flight before the handoff: the worker clears `pending` right
        // after recv(), so storing it after try_send can be overwritten and wedge
        // busy() true forever.
        self.pending.store(true, Ordering::Release);
        self.liveness.mark_sent();
        if tx.try_send(OutputFrame::Cpu(buffer)).is_err() {
            self.pending.store(false, Ordering::Release);
            self.liveness.clear_sent();
            self.stats.lock().unwrap().frames_dropped += 1;
        }
    }

    fn name(&self) -> &str {
        return &self.name;
    }

    fn enabled(&self) -> bool {
        return self.enabled.load(Ordering::Relaxed);
    }

    fn busy(&self) -> bool {
        return self.pending.load(Ordering::Acquire) && self.liveness.is_alive();
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn stats(&self) -> Arc<Mutex<OutputStats>> {
        return self.stats.clone();
    }

    fn protocol(&self) -> Protocol {
        return Protocol::Decklink;
    }

    fn shutdown(&mut self) {
        self.stop_worker(RESTART_GRACE);
    }
}

impl DecklinkOutput {
    fn stop_worker(&mut self, grace: Duration) {
        drop(self.frame_tx.take());
        let Some(thread) = self.thread.take() else {
            return;
        };
        // The worker can be inside a blocking DeckLink call (device
        // enumeration, EnableVideoOutput, StopScheduledPlayback) or in its start
        // retry, so the engine thread must not join it blindly: with a configured
        // device that is absent, that is seconds of frozen UI on a toggle.
        // The grace covers the normal path, which keeps the device released
        // before a restart opens it again; past it the worker finishes alone and
        // releases its own handle.
        let deadline = Instant::now() + grace;
        while self.liveness.is_alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if self.liveness.is_alive() {
            tracing::debug!(output = self.name, "DeckLink output worker still releasing the device; detaching");
            // The detached worker may still read a CV surface or a mapped
            // readback slot; leak those instead of freeing them under it.
            #[cfg(target_os = "macos")]
            {
                let ring = std::mem::take(self.cv_ring.get_mut().unwrap());
                std::mem::forget(ring);
            }
            let pool = std::mem::replace(self.readback.get_mut().unwrap(), ReadbackPool::new());
            std::mem::forget(pool);
            return;
        }
        let _ = thread.join();
    }
}

impl Drop for DecklinkOutput {
    fn drop(&mut self) {
        self.stop_worker(SHUTDOWN_GRACE);
    }
}

#[cfg(test)]
mod tests {
    use super::{CV_TOKEN_BIT, PINNED_TOKEN_BIT, TokenKind, classify_token};

    #[test]
    fn classifies_completion_tokens() {
        assert_eq!(classify_token(0), TokenKind::Vec);
        assert_eq!(classify_token(7), TokenKind::Vec);
        assert_eq!(classify_token(CV_TOKEN_BIT | 2), TokenKind::Cv(2));
        assert_eq!(classify_token(PINNED_TOKEN_BIT | 1), TokenKind::Pinned(1));
    }
}
