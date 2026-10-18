use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ironrdp_client::config::{ConfigBuilder, Destination};
use ironrdp_client::rdp::{RdpClient, RdpOutputEvent};
use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::secret;
use crate::APP_NAME;
use super::super::{
    CpuFrame, Frame, FramePool, PixelFormat, SourceRef, SourceStats, VideoSource,
};

/// Default RDP port.
const DEFAULT_PORT: u16 = 3389;
/// Desktop resolution the session asks the host for, matching the engine's own
/// defaults. A host may keep the resolution a session already runs at, so a
/// change here only takes effect on a fresh session.
const DEFAULT_DESKTOP_WIDTH: u16 = 1280;
const DEFAULT_DESKTOP_HEIGHT: u16 = 720;
/// Bounds on the requested resolution, so a hand-edited project cannot ask for
/// a desktop the session cannot be built with. RDP has no documented minimum
/// and only clamps values above 8192, so the floor only guards against a typo.
const MIN_DESKTOP_SIDE: u16 = 16;
const MAX_DESKTOP_SIDE: u16 = 8192;
/// RDP pixels are 32-bit; the compositor samples them as BGRA.
const BGRA_BYTES: usize = 4;
/// Client identity reported to the host. It is informational, but the engine
/// refuses to build a connection while any of the four fields is unset.
const CLIENT_NAME: &str = APP_NAME;
const CLIENT_DIR: &str = "C:\\";
const CLIENT_BUILD: u32 = 0;
/// Frames the engine may queue, each a whole desktop. A shallow queue keeps a
/// slow consumer from making the engine buffer several copies, and the session
/// only ever publishes the newest.
const FRAME_QUEUE: usize = 2;
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Granularity of the retry sleep, so a stop request is seen promptly.
const RETRY_SLICE: Duration = Duration::from_millis(50);
/// How long the session waits for an event before it re-checks `running` and
/// its own publish timer, so a stop request or the tail of a burst is never
/// held up while the host is silent.
const IDLE_POLL: Duration = Duration::from_millis(25);
/// Quiet time that marks the end of a burst of updates, so one screenful of
/// damage publishes as one frame instead of several partial ones.
const SETTLE: Duration = Duration::from_millis(16);
/// Floor between two published frames; a full frame is a whole-desktop copy.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(16);
/// Never hold a dirty frame longer than this, even mid-stream.
const MAX_PUBLISH_INTERVAL: Duration = Duration::from_millis(33);

// @docs protocol="RDP" direction=input
#[derive(Clone, PartialEq)]
pub struct RdpSourceConfig {
    // @docs label="Host" default=""
    /// Server address: an IP literal or a hostname.
    pub host: String,
    // @docs label="Port" default="3389"
    /// RDP port; 3389 unless the server was moved.
    pub port: u16,
    // @docs label="Username" default=""
    /// Account name, or the full user principal name when the server wants a
    /// UPN login.
    pub username: String,
    // @docs label="Domain" default=""
    /// Windows domain, or empty for a local account or a UPN login.
    pub domain: String,
    // @docs label="Password" default=""
    /// The account password. Stored encrypted with a key built into the app.
    pub password: String,
    // @docs default="1280"
    /// Requested desktop width in pixels. The host may keep the resolution of a
    /// session that is already running, so a change applies to a fresh session.
    pub desktop_width: u16,
    // @docs default="720"
    /// Requested desktop height in pixels.
    pub desktop_height: u16,
    // @docs label="Color depth" options="16-bit,32-bit" default="32-bit"
    /// Channel depth requested from the host. 16-bit halves what comes over the
    /// wire and what the host has to encode, at the cost of visible banding on gradients
    pub color_depth: RdpColorDepth,
}

/// Channel depth the session asks the host to render at.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RdpColorDepth {
    Bpp16,
    #[default]
    Bpp32,
}

impl RdpColorDepth {
    pub fn label(self) -> &'static str {
        return match self {
            RdpColorDepth::Bpp16 => "16-bit",
            RdpColorDepth::Bpp32 => "32-bit",
        };
    }

    fn bits(self) -> u32 {
        return match self {
            RdpColorDepth::Bpp16 => 16,
            RdpColorDepth::Bpp32 => 32,
        };
    }
}

impl Default for RdpSourceConfig {
    fn default() -> Self {
        return Self {
            host: String::new(),
            port: DEFAULT_PORT,
            username: String::new(),
            domain: String::new(),
            password: String::new(),
            desktop_width: DEFAULT_DESKTOP_WIDTH,
            desktop_height: DEFAULT_DESKTOP_HEIGHT,
            color_depth: RdpColorDepth::default(),
        };
    }
}

impl RdpSourceConfig {
    /// Whether enough is configured to open a connection. The engine rejects a
    /// session without credentials, so those are part of the answer.
    pub fn is_configured(&self) -> bool {
        return !self.host.trim().is_empty()
            && !self.username.is_empty()
            && !self.password.is_empty();
    }

    /// Label for source lists: `host:port`, or the source's own name while no
    /// host is configured.
    pub fn label(&self, source_ref: &str) -> String {
        if self.host.trim().is_empty() {
            return source_ref.to_string();
        }
        return format!("{}:{}", self.host.trim(), self.port);
    }

    /// Resolution to ask the host for, held inside the range a session can be
    /// built with.
    pub fn desktop_size(&self) -> (u16, u16) {
        return (
            self.desktop_width.clamp(MIN_DESKTOP_SIDE, MAX_DESKTOP_SIDE),
            self.desktop_height.clamp(MIN_DESKTOP_SIDE, MAX_DESKTOP_SIDE),
        );
    }
}

/// The password never reaches a log through `Debug`.
impl std::fmt::Debug for RdpSourceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let password = if self.password.is_empty() { "" } else { "<redacted>" };
        return f
            .debug_struct("RdpSourceConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("domain", &self.domain)
            .field("password", &password)
            .field("desktop_width", &self.desktop_width)
            .field("desktop_height", &self.desktop_height)
            .field("color_depth", &self.color_depth)
            .finish();
    }
}

fn default_port() -> u16 {
    return DEFAULT_PORT;
}

fn default_desktop_width() -> u16 {
    return DEFAULT_DESKTOP_WIDTH;
}

fn default_desktop_height() -> u16 {
    return DEFAULT_DESKTOP_HEIGHT;
}

#[derive(Deserialize)]
struct RdpSourceConfigWire {
    #[serde(default)]
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    username: String,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    password: String,
    #[serde(default = "default_desktop_width")]
    desktop_width: u16,
    #[serde(default = "default_desktop_height")]
    desktop_height: u16,
    #[serde(default)]
    color_depth: RdpColorDepth,
}

impl Serialize for RdpSourceConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("RdpSourceConfig", 8)?;
        state.serialize_field("host", &self.host)?;
        state.serialize_field("port", &self.port)?;
        state.serialize_field("username", &self.username)?;
        state.serialize_field("domain", &self.domain)?;
        state.serialize_field("password", &secret::encrypt(&self.password))?;
        state.serialize_field("desktop_width", &self.desktop_width)?;
        state.serialize_field("desktop_height", &self.desktop_height)?;
        state.serialize_field("color_depth", &self.color_depth)?;
        return state.end();
    }
}

impl<'de> Deserialize<'de> for RdpSourceConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = RdpSourceConfigWire::deserialize(deserializer)?;
        return Ok(Self {
            host: wire.host,
            port: wire.port,
            username: wire.username,
            domain: wire.domain,
            password: secret::decrypt(&wire.password),
            desktop_width: wire.desktop_width,
            desktop_height: wire.desktop_height,
            color_depth: wire.color_depth,
        });
    }
}

/// Active RDP session on its own thread.
pub struct RdpSource {
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    source_ref: SourceRef,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RdpSource {
    pub fn spawn(source_ref: SourceRef, config: &RdpSourceConfig) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let running = Arc::new(AtomicBool::new(true));

        let slot2 = slot.clone();
        let stats2 = stats.clone();
        let running2 = running.clone();
        let config = config.clone();
        let thread_ref = source_ref.clone();

        let thread = std::thread::Builder::new()
            .name(format!("rdp-in-{source_ref}"))
            .spawn(move || run_capture(config, slot2, stats2, running2, thread_ref))
            .expect("spawn rdp-recv");
        return Self {
            slot,
            stats,
            source_ref,
            running,
            thread: Some(thread),
        };
    }
}

/// Connect and publish frames until `running` clears, reconnecting on failure.
fn run_capture(
    config: RdpSourceConfig,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    source_ref: SourceRef,
) {
    if !config.is_configured() {
        tracing::warn!(source = source_ref, "RDP source is not configured");
        return;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!(source = source_ref, "RDP runtime init failed: {e}");
            return;
        }
    };
    runtime.block_on(run_session(config, slot, stats, running, source_ref));
}

async fn run_session(
    config: RdpSourceConfig,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    source_ref: SourceRef,
) {
    let label = config.label(&source_ref);
    let mut seq = 0u64;
    let mut pool = FramePool::new();

    while running.load(Ordering::Relaxed) {
        let (frames_tx, mut frames) = mpsc::channel::<RdpOutputEvent>(FRAME_QUEUE);
        let client = match client(&config, frames_tx) {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!(source = source_ref, "RDP config rejected: {e}");
                sleep_while_running(&running, RETRY_DELAY).await;
                continue;
            }
        };
        tracing::info!(source = source_ref, "RDP connecting to {label}");

        let mut latest: Option<(Vec<u32>, u32, u32)> = None;
        let mut dirty = false;
        let mut last_event_at = Instant::now();
        let mut last_publish_at = Instant::now();

        // The client runs alongside our own loop on this thread: its future is
        // not `Send`, and the loop has to notice a stop request even while the
        // host is silent.
        tokio::select! {
            _ = client.run() => {
                tracing::debug!(source = source_ref, "RDP session finished");
            }
            _ = async {
                loop {
                    if !running.load(Ordering::Relaxed) {
                        return;
                    }

                    tokio::select! {
                        event = frames.recv() => match event {
                            Some(RdpOutputEvent::Image { buffer, width, height }) => {
                                latest = Some((buffer, width.get() as u32, height.get() as u32));
                                dirty = true;
                                last_event_at = Instant::now();
                            }
                            Some(RdpOutputEvent::ConnectionFailure(e)) => {
                                tracing::warn!(source = source_ref, "RDP connection failed: {e}");
                                return;
                            }
                            Some(RdpOutputEvent::Terminated(result)) => {
                                tracing::warn!(source = source_ref, "RDP session ended: {result:?}");
                                return;
                            }
                            // Pointer events carry the cursor separately for a
                            // client that draws one itself; software rendering
                            // already puts it in the frame.
                            Some(_) => {}
                            None => return,
                        },
                        _ = tokio::time::sleep(IDLE_POLL) => {}
                    }

                    if dirty
                        && publish_due(last_event_at.elapsed(), last_publish_at.elapsed())
                        && let Some((buffer, w, h)) = latest.take()
                    {
                        let frame_span = tracing::debug_span!("rdp_frame");
                        let _frame_guard = frame_span.entered();
                        let started = Instant::now();
                        let mut data = pool.take(w as usize * h as usize * BGRA_BYTES);
                        to_bgra(&buffer, &mut data);
                        let copy_ms = started.elapsed().as_secs_f32() * 1000.0;
                        {
                            let mut s = stats.lock().unwrap();
                            s.record_frame(w, h, PixelFormat::Bgra8.label(), 0.0);
                            s.record_copy_time(copy_ms);
                        }
                        let old = slot.lock().unwrap().replace(Frame::Cpu(CpuFrame {
                            data: Arc::new(data),
                            w,
                            h,
                            fmt: PixelFormat::Bgra8,
                            pitch: 0,
                            seq,
                        }));
                        pool.give(old);
                        seq += 1;
                        dirty = false;
                        last_publish_at = Instant::now();
                    }
                }
            } => {}
        }

        if !running.load(Ordering::Relaxed) {
            return;
        }
        tracing::info!(source = source_ref, "RDP reconnecting to {label}");
        sleep_while_running(&running, RETRY_DELAY).await;
    }
}

/// Build the session. TLS and CredSSP are on by default; the certificate is
/// always accepted, which is what makes a host serving its own self-signed one
/// connectable at all.
fn client(config: &RdpSourceConfig, frames: mpsc::Sender<RdpOutputEvent>) -> anyhow::Result<RdpClient> {
    let (desktop_width, desktop_height) = config.desktop_size();
    let engine = ConfigBuilder::new()
        .with_destination(Destination::from_parts(config.host.trim().to_string(), config.port))
        .with_username(config.username.clone())
        .with_domain(config.domain.clone())
        .with_password(config.password.clone())
        .with_client_name(CLIENT_NAME)
        .with_client_dir(CLIENT_DIR)
        .with_client_build(CLIENT_BUILD)
        .with_platform(MajorPlatformType::UNSPECIFIED)
        .with_desktop_width(desktop_width)
        .with_desktop_height(desktop_height)
        .with_color_depth(config.color_depth.bits())
        // Headless: ask for a plain desktop-sized framebuffer and let the
        // session draw the cursor into it.
        .with_pointer_software_rendering(true)
        .build()?;
    return Ok(RdpClient::new(engine, frames));
}

/// Convert a tightly packed `0x00RRGGBB` desktop into the BGRA bytes the
/// compositor samples. RDP leaves the fourth byte zero, and the canvas an output
/// reads would carry that alpha through, so it is forced opaque.
fn to_bgra(pixels: &[u32], out: &mut [u8]) {
    for (pixel, quad) in pixels.iter().zip(out.chunks_exact_mut(BGRA_BYTES)) {
        quad[0] = *pixel as u8;
        quad[1] = (*pixel >> 8) as u8;
        quad[2] = (*pixel >> 16) as u8;
        quad[3] = 0xff;
    }
}

/// Sleep for `duration`, returning early once `running` clears, so the source
/// thread joins promptly instead of waiting out a retry.
async fn sleep_while_running(running: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while running.load(Ordering::Relaxed) && Instant::now() < deadline {
        let slice = RETRY_SLICE.min(deadline.saturating_duration_since(Instant::now()));
        tokio::time::sleep(slice).await;
    }
}

/// Whether to publish. A settled batch is a whole screen; the fallback stops a
/// host that never pauses from freezing the tile.
fn publish_due(quiet: Duration, since_publish: Duration) -> bool {
    if since_publish < PUBLISH_INTERVAL {
        return false;
    }
    return quiet >= SETTLE || since_publish >= MAX_PUBLISH_INTERVAL;
}

impl Drop for RdpSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take()
            && let Err(e) = t.join()
        {
            tracing::error!(source = self.source_ref, "Thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for RdpSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        return self.slot.lock().unwrap().clone();
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RDP pixels are `0x00RRGGBB`, so a pixel has to land as bytes in B, G, R
    /// order with alpha forced opaque.
    #[test]
    fn converts_pixels_to_opaque_bgra() {
        let mut out = vec![0u8; 8];
        to_bgra(&[0x0012_3456, 0x00ff_0000], &mut out);
        assert_eq!(out, vec![0x56, 0x34, 0x12, 0xff, 0x00, 0x00, 0xff, 0xff]);
    }

    #[test]
    fn is_configured_needs_host_and_credentials() {
        let host_only = RdpSourceConfig {
            host: "studio".to_string(),
            ..Default::default()
        };
        assert!(!host_only.is_configured());
        let credentials = RdpSourceConfig {
            username: "operator".to_string(),
            password: "secret".to_string(),
            ..host_only
        };
        assert!(credentials.is_configured());
    }

    #[test]
    fn label_prefers_host_and_port() {
        assert_eq!(RdpSourceConfig::default().label("RDP 1"), "RDP 1");
        let configured = RdpSourceConfig {
            host: " studio ".to_string(),
            port: 3390,
            ..Default::default()
        };
        assert_eq!(configured.label("RDP 1"), "studio:3390");
    }

    /// A project saved before a field existed still opens, and the port and
    /// desktop keep their protocol defaults rather than becoming zero.
    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let parsed: RdpSourceConfig = serde_json::from_str(r#"{"host":"studio"}"#).unwrap();
        assert_eq!(parsed.port, 3389);
        assert!(parsed.username.is_empty());
        assert!(parsed.domain.is_empty());
        assert_eq!(parsed.desktop_size(), (1280, 720));
        assert_eq!(parsed.color_depth, RdpColorDepth::Bpp32);
    }

    /// A hand-edited resolution cannot reach the session as a size it could not
    /// be built with.
    #[test]
    fn desktop_size_is_held_inside_its_bounds() {
        let too_small = RdpSourceConfig {
            desktop_width: 0,
            desktop_height: 1,
            ..Default::default()
        };
        assert_eq!(too_small.desktop_size(), (16, 16));

        let too_large = RdpSourceConfig {
            desktop_width: u16::MAX,
            desktop_height: 9000,
            ..Default::default()
        };
        assert_eq!(too_large.desktop_size(), (8192, 8192));
    }

    /// A host that refuses the connection is retried in the background, and the
    /// source still stops on `Drop` instead of holding up the thread join.
    #[test]
    fn stops_cleanly_when_the_host_is_unreachable() {
        let config = RdpSourceConfig {
            host: "127.0.0.1".to_string(),
            port: 1,
            username: "operator".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let source = RdpSource::spawn("RDP 1".to_string(), &config);
        std::thread::sleep(Duration::from_millis(300));
        drop(source);
    }
}
