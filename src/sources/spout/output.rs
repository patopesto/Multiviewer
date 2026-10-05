use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};
use spout2::dx12::resource_state::COPY_DEST;
use wgpu::hal::api::Dx12 as Dx12Api;

use super::super::{OutputStats, Protocol};
use super::super::output::{OutputId, VideoOutput};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SpoutOutputConfig {
    pub sender_name: String,
}

/// Owns the Spout sender, its intermediate texture and the D3D11On12 wrapper.
///
/// Field order is drop order: the wrapper borrows both the texture and the
/// sender's device, so it must be released first.
///
/// `unsafe impl Send`: all three handles are only touched while holding the
/// owning `SpoutOutput::state` mutex, from the render thread.
struct SendState {
    wrapped: spout2::dx12::WrappedResource,
    texture: wgpu::Texture,
    sender: spout2::dx12::Sender,
    width: u32,
    height: u32,
}
unsafe impl Send for SendState {}

/// Zero-copy Spout sender: the canvas is GPU-copied into a dedicated texture
/// (isolating Spout from the resource-state changes other outputs make to the
/// canvas), then shared through Spout's D3D11On12 bridge.
pub struct SpoutOutput {
    #[allow(dead_code)]
    id: OutputId,
    name: String,
    config: SpoutOutputConfig,
    enabled: AtomicBool,
    stats: Arc<Mutex<OutputStats>>,
    state: Mutex<Option<SendState>>,
    diagnostics_logged: AtomicBool, // Sender open is retried every frame; the first failure is logged once instead of at 60 Hz.
}

impl SpoutOutput {
    /// `name` is the output's display name; a non-empty `config.sender_name`
    /// overrides it as the Spout sender name.
    pub fn new(id: OutputId, name: String, config: SpoutOutputConfig, enabled: bool) -> Self {
        Self {
            id,
            name,
            config,
            enabled: AtomicBool::new(enabled),
            stats: Arc::new(Mutex::new(OutputStats::default())),
            state: Mutex::new(None),
            diagnostics_logged: AtomicBool::new(false),
        }
    }

    /// Open the Spout sender sharing wgpu's D3D12 device and command queue.
    fn open_sender(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sender_name: &str,
    ) -> Result<spout2::dx12::Sender, String> {
        use windows::core::Interface;
        // Safety: device/queue outlive the sender; the app owns both for its lifetime.
        unsafe {
            let Some(hal_device) = device.as_hal::<Dx12Api>() else {
                return Err("Spout requires wgpu's D3D12 backend".to_string());
            };
            let Some(hal_queue) = queue.as_hal::<Dx12Api>() else {
                return Err("Spout requires wgpu's D3D12 backend".to_string());
            };
            let device_ptr = hal_device.raw_device().as_raw();
            let mut queue_ptr = hal_queue.as_raw().as_raw();
            spout2::dx12::Sender::with_device(sender_name, device_ptr, &mut queue_ptr)
                .map_err(|e| e.to_string())
        }
    }

    /// The raw `ID3D12Resource*` backing a wgpu texture, for Spout's wrap.
    fn texture_ptr(texture: &wgpu::Texture) -> Result<*mut c_void, &'static str> {
        use windows::core::Interface;
        // Safety: the texture is alive for this call and shares the sender's device.
        unsafe {
            let Some(hal_texture) = texture.as_hal::<Dx12Api>() else {
                return Err("Spout requires wgpu's D3D12 backend");
            };
            Ok(hal_texture.raw_resource().as_raw())
        }
    }

    /// Create the sender, the intermediate texture and its Spout wrapper.
    fn create_state(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sender_name: &str,
        width: u32,
        height: u32,
    ) -> Result<SendState, String> {
        let sender = Self::open_sender(device, queue, sender_name)?;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(sender_name),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let ptr = Self::texture_ptr(&texture)?;
        // Safety: the texture lives on the sender's device; the copy below leaves it in `COPY_DEST`, the state `send_wrapped_resource` requires.
        let wrapped = unsafe {
            sender.wrap_resource(ptr, COPY_DEST)
        }
        .map_err(|e| e.to_string())?;
        Ok(SendState {
            wrapped,
            texture,
            sender,
            width,
            height,
        })
    }

    pub fn present(
        &self,
        texture: &wgpu::Texture,
        _width: u32,
        _height: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        let width = texture.width();
        let height = texture.height();
        let mut state_guard = self.state.lock().unwrap();
        let recreate = state_guard
            .as_ref()
            .map(|s| s.width != width || s.height != height)
            .unwrap_or(true);

        if recreate {
            match Self::create_state(device, queue, &self.config.sender_name, width, height) {
                Ok(state) => {
                    tracing::info!(output=self.name, "Spout output created: ({}x{})", width, height);
                    *state_guard = Some(state);
                    let mut s = self.stats.lock().unwrap();
                    s.width = width;
                    s.height = height;
                }
                Err(e) => {
                    *state_guard = None;
                    // Retried every frame; log the first failure only.
                    if !self.diagnostics_logged.swap(true, Ordering::Relaxed) {
                        tracing::error!(output=self.name, "Spout output open failed: {}", e);
                    }
                    return;
                }
            }
        }
        let state = state_guard.as_mut().unwrap();

        // GPU copy into the intermediate texture. This submits on wgpu's command
        // queue, which Spout shares (passed to `with_device`), so the send below
        // is ordered after it.
        {
            let copy_span = tracing::debug_span!("copy");
            let _copy_guard = copy_span.entered();
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("spout-output"),
            });
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &state.texture,
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

        let send_span = tracing::debug_span!("send");
        let _send_guard = send_span.entered();
        let send_start = std::time::Instant::now();
        // Safety: `wrapped` was created from this sender and the copy above left the texture in `COPY_DEST`.
        let result = unsafe { state.sender.send_wrapped_resource(&state.wrapped) };
        let mut s = self.stats.lock().unwrap();
        s.send_time_ms = send_start.elapsed().as_secs_f32() * 1000.0;
        match result {
            Ok(()) => s.record_sent(),
            Err(e) => {
                s.frames_dropped += 1;
                tracing::error!(output=self.name, "Spout send failed: {}", e);
            }
        }
    }
}

impl VideoOutput for SpoutOutput {
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

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn stats(&self) -> Arc<Mutex<OutputStats>> {
        self.stats.clone()
    }

    fn protocol(&self) -> Protocol {
        Protocol::Spout
    }
}
