use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use openmediatransport::{
    ColorSpace as OmtColorSpace, Discovery, FrameType, GpuVideoContext, Sender, SenderInfo,
    VideoFlags, VideoTextureMeta,
};
use serde::{Deserialize, Serialize};

use super::super::{OutputStats, Protocol};
use super::super::output::{OutputId, OutputLiveness, VideoOutput};
use crate::sources::{
    ColorSpace as AppColorSpace, PixelFormat, TextureTarget, UPLOAD_TARGET_USAGE,
};

// @docs protocol="OMT" direction=output
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct OmtOutputConfig {
    // @docs hidden
    /// Name advertised on the network. Empty uses the output's name.
    pub sender_name: String,
}

/// How long the sender thread waits for a frame before polling the socket again.
const POLL_TIMEOUT: Duration = Duration::from_millis(2);

/// Frames that may be queued for the sender.
const OMT_QUEUE_DEPTH: usize = 2;

/// Reusable GPU textures so the encoder never reads a texture the main thread
/// is already overwriting.
const OMT_TEXTURE_POOL: usize = 3;

/// Zero-copy OMT sender: the canvas is GPU-copied into a dedicated texture
/// (isolating the sender from the canvas the compositor re-renders every frame),
/// then encoded and advertised by a worker thread that owns the `Sender`.
pub struct OmtOutput {
    name: String,
    config: OmtOutputConfig,
    enabled: AtomicBool,
    stats: Arc<Mutex<OutputStats>>,
    liveness: OutputLiveness,
    subscribers: Arc<AtomicUsize>,
    free: Mutex<Vec<Arc<TextureTarget>>>,
    free_rx: Mutex<Option<mpsc::Receiver<Arc<TextureTarget>>>>,
    in_flight: AtomicUsize,
    frame_tx: Mutex<Option<mpsc::SyncSender<Arc<TextureTarget>>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl OmtOutput {
    pub fn new(_id: OutputId, name: String, config: OmtOutputConfig, enabled: bool) -> Self {
        return Self {
            name,
            config,
            enabled: AtomicBool::new(enabled),
            stats: Arc::new(Mutex::new(OutputStats::default())),
            liveness: OutputLiveness::default(),
            subscribers: Arc::new(AtomicUsize::new(0)),
            free: Mutex::new(Vec::new()),
            free_rx: Mutex::new(None),
            in_flight: AtomicUsize::new(0),
            frame_tx: Mutex::new(None),
            thread: Mutex::new(None),
        };
    }

    /// Start the sender thread on the first present, when the device is known.
    fn ensure_started(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut thread = self.thread.lock().unwrap();
        if thread.is_some() {
            return;
        }
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Arc<TextureTarget>>(OMT_QUEUE_DEPTH);
        *self.frame_tx.lock().unwrap() = Some(frame_tx);
        let (free_tx, free_rx) = mpsc::channel::<Arc<TextureTarget>>();
        *self.free_rx.lock().unwrap() = Some(free_rx);

        let ctx = GpuVideoContext {
            device: Arc::new(device.clone()),
            queue: Arc::new(queue.clone()),
            gpu_lock: None,
        };
        let worker_flag = self.liveness.exit_flag();
        let stats = self.stats.clone();
        let subscribers = self.subscribers.clone();
        let sender_name = if self.config.sender_name.is_empty() {
            self.name.clone()
        } else {
            self.config.sender_name.clone()
        };
        let trace_name = self.name.clone();
        *thread = Some(
            std::thread::Builder::new()
                .name(format!("omt-out-{sender_name}"))
                .spawn(move || {
                    let _alive_guard = worker_flag;
                    run_sender(ctx, sender_name, frame_rx, free_tx, stats, subscribers, trace_name);
                })
                .expect("spawn omt sender"),
        );
    }
}

/// Own the OMT sender: accept receivers, advertise the source, and encode each
/// signalled canvas frame on the GPU.
fn run_sender(
    ctx: GpuVideoContext,
    sender_name: String,
    frame_rx: mpsc::Receiver<Arc<TextureTarget>>,
    free_tx: mpsc::Sender<Arc<TextureTarget>>,
    stats: Arc<Mutex<OutputStats>>,
    subscribers: Arc<AtomicUsize>,
    name: String,
) {
    let mut sender = match Sender::create(sender_name.as_str(), FrameType::VIDEO | FrameType::METADATA)
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(output=name, "OMT sender creation failed: {}", e);
            return;
        }
    };
    sender.set_sender_info(SenderInfo::new(
        crate::APP_NAME,
        env!("CARGO_PKG_AUTHORS"),
        env!("CARGO_PKG_VERSION"),
    ));
    // Hold one frame in the GPU encoder so this frame's FDCT overlaps the
    // previous frame's entropy coding; the last frame is flushed on disconnect.
    sender.set_gpu_encode_pipeline(true);

    let mut discovery = match Discovery::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(output=name, "OMT discovery init failed: {}", e);
            return;
        }
    };
    if let Err(e) = discovery.register(&sender_name, sender.port()) {
        tracing::error!(output=name, "OMT register failed: {}", e);
        return;
    }
    tracing::info!(
        output=name,
        "OMT output advertised as '{}' on port {}",
        sender_name,
        sender.port()
    );

    loop {
        // Accept new receivers and read their subscribe/tally metadata even when
        // no frame is due, so a consumer can connect to an idle canvas.
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        subscribers.store(sender.video_subscriber_count(), Ordering::Relaxed);

        match frame_rx.recv_timeout(POLL_TIMEOUT) {
            Ok(target) => {
                let size = &target.layout;
                if sender.video_subscribed() {
                    let color_space = match AppColorSpace::for_output_height(size.height) {
                        AppColorSpace::Bt601 => OmtColorSpace::Bt601,
                        AppColorSpace::Bt709 => OmtColorSpace::Bt709,
                    };
                    let meta = VideoTextureMeta {
                        width: size.width,
                        height: size.height,
                        timestamp: -1,
                        frame_rate_n: 60,
                        frame_rate_d: 1,
                        color_space,
                        flags: VideoFlags::NONE,
                        frame_metadata: None,
                    };
                    let send_span = tracing::debug_span!("send");
                    let _send_guard = send_span.entered();
                    let start = Instant::now();
                    let result = sender.send_video_texture(&ctx, &target.texture, meta);
                    {
                        let mut s = stats.lock().unwrap();
                        s.width = size.width;
                        s.height = size.height;
                        s.send_time_ms = start.elapsed().as_secs_f32() * 1000.0;
                        if result.is_ok() {
                            s.record_sent();
                        } else {
                            s.frames_dropped += 1;
                        }
                    }
                    if let Err(e) = result {
                        tracing::error!(output=name, "OMT send failed: {}", e);
                    }
                }
                // The encoder submission is now on the queue; the texture can be
                // reused by the main thread for a later frame.
                let _ = free_tx.send(target);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Err(e) = sender.flush_gpu_encode(&ctx) {
                    tracing::error!(output=name, "OMT flush failed: {}", e);
                }
                break;
            }
        }
    }

    discovery.deregister_all().ok();
}

impl VideoOutput for OmtOutput {
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
        self.liveness.check_stall(&self.name);
        self.ensure_started(device, queue);
        // No consumer: skip the GPU copy. The worker keeps polling the socket,
        // so a receiver can still discover and connect.
        if self.subscribers.load(Ordering::Relaxed) == 0 {
            return false;
        }

        let layout = PixelFormat::Bgra8.texture_layout(width, height);
        let mut free = self.free.lock().unwrap();
        if let Some(rx) = self.free_rx.lock().unwrap().as_ref() {
            while let Ok(target) = rx.try_recv() {
                self.in_flight.fetch_sub(1, Ordering::AcqRel);
                free.push(target);
            }
        }
        free.retain(|target| target.layout == layout);
        let target = if let Some(index) = free.iter().position(|target| target.layout == layout) {
            free.swap_remove(index)
        } else if free.len() + self.in_flight.load(Ordering::Acquire) < OMT_TEXTURE_POOL {
            let mut slot = None;
            TextureTarget::ensure(&mut slot, device, layout, UPLOAD_TARGET_USAGE, "omt-output");
            Arc::new(slot.unwrap())
        } else {
            return false;
        };

        // GPU copy into the pooled texture; the worker's encode reads it later
        // on the same queue, so the send is ordered after this copy.
        {
            let copy_span = tracing::debug_span!("copy");
            let _copy_guard = copy_span.entered();
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("omt-output"),
            });
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &target.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit(Some(encoder.finish()));
        }

        let Some(tx) = self.frame_tx.lock().unwrap().clone() else {
            free.push(target);
            return false;
        };
        match tx.try_send(target) {
            Ok(()) => {
                self.in_flight.fetch_add(1, Ordering::AcqRel);
                self.liveness.mark_sent();
                return true;
            }
            Err(mpsc::TrySendError::Full(target)) | Err(mpsc::TrySendError::Disconnected(target)) => {
                free.push(target);
                self.liveness.clear_sent();
                self.stats.lock().unwrap().frames_dropped += 1;
                return false;
            }
        }
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
        return Protocol::Omt;
    }
}

impl Drop for OmtOutput {
    fn drop(&mut self) {
        // Drop the sender before joining, or the worker's recv_timeout never
        // sees the channel disconnect and the join deadlocks.
        drop(self.frame_tx.lock().unwrap().take());
        if let Some(thread) = self.thread.lock().unwrap().take()
            && let Err(e) = thread.join()
        {
            tracing::error!(output=self.name, "Thread join failed: {:?}", e);
        }
    }
}
