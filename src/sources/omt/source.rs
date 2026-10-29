use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use openmediatransport::{
    ColorSpace, FrameType, GpuVideoContext, Quality, ReceiverConfig, ReceiverSession, SessionState,
};
use serde::{Deserialize, Serialize};

use super::OmtSourceInfo;
use super::super::{
    ConvUniform, Frame, GpuFrame, PixelFormat, SourceRef, SourceStats, VideoSource,
};

// @docs protocol="OMT" direction=input
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OmtSourceConfig {
    // @docs label="Quality" options="Default,Low,Medium,High"
    /// Encoder quality requested from the sender.
    pub quality: OmtQuality,
    // @docs label="Preview"
    /// Receive 1/8-resolution preview frames instead of full resolution.
    pub preview: bool,
}

impl Default for OmtSourceConfig {
    fn default() -> Self {
        return Self {
            quality: OmtQuality::Default,
            preview: false,
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OmtQuality {
    Default,
    Low,
    Medium,
    High,
}

impl OmtQuality {
    fn to_native(self) -> Quality {
        return match self {
            OmtQuality::Default => Quality::Default,
            OmtQuality::Low => Quality::Low,
            OmtQuality::Medium => Quality::Medium,
            OmtQuality::High => Quality::High,
        };
    }
}

/// Active OMT receiver on its own thread, decoding on the GPU.
pub struct OmtSource {
    source_ref: SourceRef,
    discovered: OmtSourceInfo,
    config: OmtSourceConfig,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl OmtSource {
    pub fn spawn(source_ref: SourceRef, discovered: OmtSourceInfo, config: &OmtSourceConfig) -> Self {
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        stats.lock().unwrap().set_gpu_direct();
        return Self {
            source_ref,
            discovered,
            config: config.clone(),
            slot: Arc::new(Mutex::new(None)),
            stats,
            running: Arc::new(AtomicBool::new(true)),
            thread: Mutex::new(None),
        };
    }

    /// Start the capture thread on the first frame pull, when the GPU device is
    /// known. The connect is a synchronous TCP connect, so it never runs on the
    /// render thread.
    fn ensure_started(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut thread = self.thread.lock().unwrap();
        if thread.is_some() {
            return;
        }
        let ctx = GpuVideoContext {
            device: Arc::new(device.clone()),
            queue: Arc::new(queue.clone()),
            gpu_lock: None,
        };
        let slot = self.slot.clone();
        let stats = self.stats.clone();
        let running = self.running.clone();
        let discovered = self.discovered.clone();
        let config = self.config.clone();
        let trace_ref = self.source_ref.clone();
        *thread = Some(
            std::thread::Builder::new()
                .name(format!("omt-in-{}", self.source_ref))
                .spawn(move || {
                    run_capture(discovered, config, ctx, slot, stats, running, trace_ref);
                })
                .expect("spawn omt-recv"),
        );
    }
}

/// Poll timeout for the decoded-frame queue.
const POLL_TIMEOUT: Duration = Duration::from_millis(100);

/// How long the feed may go without a decoded frame before the decoder is
/// restarted. OMT senders stream continuously, so a gap this long means the
/// session or decoder is stuck: the crate only reconnects on socket errors, not
/// on a sender that goes silent while the connection stays open.
const STALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Video format the decoder is currently producing. A change means the sender
/// switched resolution, frame rate, colour space or scan mode, and the cached
/// VMX codec has to be rebuilt by reconnecting.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FeedFormat {
    width: u32,
    height: u32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    color_space: ColorSpace,
}

/// True when a decoded frame's format differs from the one the decoder is
/// configured for; the first frame never counts as a change.
fn feed_changed(previous: Option<FeedFormat>, received: FeedFormat) -> bool {
    return previous.is_some_and(|previous| previous != received);
}

/// True when the decoder should be restarted: no decoded frame for more than
/// [`STALL_TIMEOUT`], or the session has given up.
fn feed_stalled(silent_for: Duration, state: SessionState) -> bool {
    return silent_for >= STALL_TIMEOUT || state == SessionState::Stopped;
}

/// Receive and publish GPU frames on the source's thread until `running`
/// clears. Reconnects — and so rebuilds the VMX decoder — whenever the feed
/// format changes or the feed stalls.
fn run_capture(
    discovered: OmtSourceInfo,
    config: OmtSourceConfig,
    ctx: GpuVideoContext,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    source_ref: SourceRef,
) {
    let layout = framebuffer_layout(&ctx.device);
    let sampler = create_sampler(&ctx.device);
    let uniform = conv_uniform(&ctx.device, &ctx.queue);
    let mut seq = 0u64;

    while running.load(Ordering::Relaxed) {
        let receiver_config = ReceiverConfig {
            frame_types: FrameType::VIDEO,
            quality: config.quality.to_native(),
            preview: config.preview,
            connect_timeout: Duration::from_secs(2),
            auto_reconnect: true,
            gpu: Some(ctx.clone()),
        };
        let session = match ReceiverSession::connect(discovered.to_url(), receiver_config) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(source=source_ref, "OMT connect failed: {}", e);
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        tracing::info!(source=source_ref, "OMT connected");

        let mut format: Option<FeedFormat> = None;
        let mut last_frame_at = Instant::now();
        let mut last_bytes = 0u64;
        while running.load(Ordering::Relaxed) {
            let frame_span = tracing::debug_span!("omt_frame");
            let _frame_guard = frame_span.entered();
            let Some(frame) = session.recv_video_gpu_timeout(POLL_TIMEOUT) else {
                if feed_stalled(last_frame_at.elapsed(), session.state()) {
                    tracing::warn!(source=source_ref, "OMT feed stalled, restarting decoder");
                    break;
                }
                continue;
            };

            let received = FeedFormat {
                width: frame.width,
                height: frame.height,
                frame_rate_n: frame.frame_rate_n,
                frame_rate_d: frame.frame_rate_d,
                color_space: frame.color_space,
            };
            if feed_changed(format, received) {
                tracing::info!(source=source_ref, "OMT feed format changed, restarting decoder");
                break;
            }
            format = Some(received);
            last_frame_at = Instant::now();

            let w = frame.width;
            let h = frame.height;
            let session_stats = session.statistics();
            let delta_bytes = session_stats.bytes_received.saturating_sub(last_bytes);
            last_bytes = session_stats.bytes_received;
            let decode_ms = if session_stats.frames_decoded > 0 {
                session_stats.codec_time_ns as f32 / 1_000_000.0
                    / session_stats.frames_decoded as f32
            } else {
                0.0
            };
            let nominal_fps = if frame.frame_rate_d > 0 {
                frame.frame_rate_n as f64 / frame.frame_rate_d as f64
            } else {
                0.0
            };
            {
                let mut s = stats.lock().unwrap();
                s.record_frame(w, h, PixelFormat::Bgra8.label(), nominal_fps);
                s.record_network(delta_bytes);
                s.set_decode_ms(decode_ms);
            }
            let bg = Arc::new(framebuffer_bind_group(
                &ctx.device,
                &layout,
                &sampler,
                &uniform,
                &frame.texture,
            ));
            let old = slot.lock().unwrap().replace(Frame::Gpu(GpuFrame {
                bg,
                w,
                h,
                seq,
                flip_v: false,
            }));
            drop(old);
            seq += 1;
        }
    }
}

/// Bind group layout for a GPU source frame: texture, sampler and conversion
/// uniform. Mirrors the compositor's `Pipeline::Main` layout.
fn framebuffer_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    return device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("omt-source"),
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
}

fn create_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    return device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
}

/// Passthrough uniform; the shader ignores width/height in this mode.
fn conv_uniform(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Buffer {
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("omt"),
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
    return uniform;
}

fn framebuffer_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    uniform: &wgpu::Buffer,
    texture: &wgpu::Texture,
) -> wgpu::BindGroup {
    let view = texture.create_view(&Default::default());
    return device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("omt"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: uniform,
                    offset: 0,
                    size: None,
                }),
            },
        ],
    });
}

impl Drop for OmtSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.lock().unwrap().take()
            && let Err(e) = t.join()
        {
            tracing::error!(source=self.source_ref, "Thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for OmtSource {
    fn latest(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Frame> {
        self.ensure_started(device, queue);
        return self.slot.lock().unwrap().clone();
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }

    fn alive(&self) -> bool {
        return self.thread.lock().unwrap().as_ref().is_none_or(|thread| !thread.is_finished());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(width: u32, frame_rate_n: i32, color_space: ColorSpace) -> FeedFormat {
        return FeedFormat {
            width,
            height: 1080,
            frame_rate_n,
            frame_rate_d: 1,
            color_space,
        };
    }

    /// The first frame establishes the format; it never counts as a change.
    #[test]
    fn first_frame_is_not_a_change() {
        assert!(!feed_changed(None, format(1920, 60, ColorSpace::Bt709)));
    }

    #[test]
    fn resolution_rate_and_colour_changes_restart() {
        let base = format(1920, 60, ColorSpace::Bt709);
        assert!(feed_changed(Some(base), format(1280, 60, ColorSpace::Bt709)));
        assert!(feed_changed(Some(base), format(1920, 50, ColorSpace::Bt709)));
        assert!(feed_changed(Some(base), format(1920, 60, ColorSpace::Bt601)));
        assert!(!feed_changed(Some(base), base));
    }

    /// A dead session restarts immediately; otherwise the decoder restarts
    /// after the feed has been silent for the stall timeout, whatever the wire
    /// is doing.
    #[test]
    fn stall_restarts_after_timeout_or_dead_session() {
        let long = STALL_TIMEOUT + Duration::from_secs(1);
        assert!(feed_stalled(long, SessionState::Connected));
        assert!(feed_stalled(long, SessionState::Reconnecting));
        assert!(feed_stalled(Duration::ZERO, SessionState::Stopped));
        assert!(!feed_stalled(STALL_TIMEOUT / 2, SessionState::Connected));
    }
}
