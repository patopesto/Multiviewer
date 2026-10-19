use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use vnc::{PixelFormat as VncPixelFormat, VncClient, VncConnector, VncEncoding, VncEvent, X11Event};

use crate::secret;
use super::super::{
    CpuFrame, Frame, FramePool, Framebuffer, PixelFormat, SourceRef, SourceStats, VideoSource,
    PUBLISH_INTERVAL, publish_due, sleep_while_running,
};

/// Default RFB port.
const DEFAULT_PORT: u16 = 5900;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Bound on the banner, security and init exchange. A server can take seconds to
/// answer while its session spins up, but a peer that accepts the connection and
/// then goes silent must not park the source thread: `Drop` joins it.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_secs(1);
/// Quiet time that marks the end of a server update. RFB has no end-of-update
/// signal the client exposes, so a server that pauses mid-update looks the same
/// as one that finished; this window is the best-effort boundary.
const SETTLE: Duration = Duration::from_millis(6);
/// Request interval while the stream is active. The server releases damage a
/// little per request, so this is short enough not to be the sweep rate.
const ACTIVE_REQUEST_INTERVAL: Duration = Duration::from_millis(4);
/// Request interval once the stream has gone quiet.
const IDLE_REQUEST_INTERVAL: Duration = Duration::from_millis(16);
/// No event for this long means the stream is idle rather than active.
const ACTIVE_WINDOW: Duration = Duration::from_millis(250);
/// How often the keep-awake moves the remote pointer.
const WAKE_INTERVAL: Duration = Duration::from_secs(10);
/// Where the keep-awake parks the remote cursor. It alternates by one pixel so
/// every event is a real move; a pointer set to where it already is may not
/// produce a motion event at all.
const WAKE_POSITIONS: [(u16, u16); 2] = [(0, 0), (1, 0)];
/// Ask for the whole screen again this long after connecting if the server has
/// sent nothing at all, so a server that ignores non-incremental requests still
/// falls back to incremental ones.
const FIRST_FRAME_DELAY: Duration = Duration::from_secs(5);
/// How long the full-screen request is given the wire before the incremental
/// cadence resumes, so a server that keeps only the latest request answers it.
const FULL_FRAME_GRACE: Duration = Duration::from_secs(1);
/// Upper bound on how long the session waits for an event before running its own
/// timers, so a stopped source is still noticed promptly.
const MAX_WAIT: Duration = Duration::from_millis(25);
/// Lower bound on that wait, so a due timer cannot turn the loop into a spin.
const MIN_WAIT: Duration = Duration::from_millis(1);

// @docs protocol="VNC" direction=input
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VncSourceConfig {
    // @docs label="Host" default=""
    /// Server address: an IP literal or a hostname.
    pub host: String,
    // @docs label="Port" default="5900"
    /// RFB port; 5900 unless the server was moved.
    pub port: u16,
    // @docs label="Password" default=""
    /// The server's VNC password, not an account password. Stored encrypted with a
    /// key built into the app.
    #[serde(serialize_with = "secret::serialize_encrypted", deserialize_with = "secret::deserialize_encrypted")]
    pub password: String,
    // @docs label="Keep awake"
    /// Send a pointer event every couple of seconds so the server does not blank an
    /// idle screen. This parks the remote cursor near the top-left and stops that
    /// machine from locking while it is idle.
    pub keep_awake: bool,
}

impl Default for VncSourceConfig {
    fn default() -> Self {
        return Self {
            host: String::new(),
            port: DEFAULT_PORT,
            password: String::new(),
            keep_awake: false,
        };
    }
}

impl VncSourceConfig {
    /// Whether enough is configured to open a connection.
    pub fn is_configured(&self) -> bool {
        return !self.host.trim().is_empty();
    }

    /// Label for source lists: `host:port`, or the source's own name while no
    /// host is configured.
    pub fn label(&self, source_ref: &str) -> String {
        if !self.is_configured() {
            return source_ref.to_string();
        }
        return format!("{}:{}", self.host.trim(), self.port);
    }
}

/// The password never reaches a log through `Debug`.
impl std::fmt::Debug for VncSourceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let password = if self.password.is_empty() { "" } else { "<redacted>" };
        return f
            .debug_struct("VncSourceConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("password", &password)
            .field("keep_awake", &self.keep_awake)
            .finish();
    }
}

/// Active RFB client on its own thread.
pub struct VncSource {
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    source_ref: SourceRef,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl VncSource {
    pub fn spawn(source_ref: SourceRef, config: &VncSourceConfig) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let running = Arc::new(AtomicBool::new(true));

        let slot2 = slot.clone();
        let stats2 = stats.clone();
        let running2 = running.clone();
        let config = config.clone();
        let trace_ref = source_ref.clone();

        let thread = std::thread::Builder::new()
            .name(format!("vnc-in-{source_ref}"))
            .spawn(move || run_capture(config, slot2, stats2, running2, trace_ref))
            .expect("spawn vnc-recv");
        return Self {
            slot,
            stats,
            source_ref,
            running,
            thread: Some(thread),
        };
    }
}

/// Request interval for a stream that has been quiet for `quiet`.
fn request_interval(quiet: Duration) -> Duration {
    if quiet < ACTIVE_WINDOW {
        return ACTIVE_REQUEST_INTERVAL;
    }
    return IDLE_REQUEST_INTERVAL;
}

/// Whether to ask for the next update. While the stream is active the server
/// hands out damage a little per request, so the interval is short; once it has
/// gone quiet a slower poll is enough and costs less.
fn request_due(quiet: Duration, since_request: Duration) -> bool {
    return since_request >= request_interval(quiet);
}

/// How long to wait for an event before running our own timers: the soonest of
/// the request and publish intervals, clamped so we never spin and never
/// oversleep.
fn next_wait(until_request: Duration, until_publish: Duration) -> Duration {
    let soonest = until_request.min(until_publish);
    return soonest.clamp(MIN_WAIT, MAX_WAIT);
}

/// Connect and publish frames until `running` clears, reconnecting on failure.
fn run_capture(
    config: VncSourceConfig,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    source_ref: SourceRef,
) {
    if !config.is_configured() {
        // Expected while a source is still being set up: its settings are in the
        // panel, and the source has to stay registered for them to be reachable.
        tracing::debug!(source = source_ref, "VNC source has no host configured");
        return;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!(source = source_ref, "VNC runtime init failed: {e}");
            return;
        }
    };
    runtime.block_on(run_session(config, slot, stats, running, source_ref));
}

async fn run_session(
    config: VncSourceConfig,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    source_ref: SourceRef,
) {
    let host = config.host.trim().to_string();
    let port = config.port;
    let label = config.label(&source_ref);
    let mut seq = 0u64;
    let mut pool = FramePool::new();

    while running.load(Ordering::Relaxed) {
        let Some(client) = connect(&host, port, &label, &config, &source_ref).await else {
            sleep_while_running(&running, RETRY_DELAY).await;
            continue;
        };
        tracing::info!(source = source_ref, "VNC connected to {label}");

        let mut framebuffer: Option<Framebuffer> = None;
        let mut pending = false;
        let mut last_event_at = Instant::now();
        let mut last_request_at = Instant::now();
        let mut last_publish_at = Instant::now();
        let mut last_wake_at = Instant::now();
        let mut wake_index = 0usize;
        // Apple's server answers nothing for the first few seconds while its
        // session spins up, and the request it should answer with the whole
        // framebuffer is the handshake's full-screen one. An incremental request
        // sent before it answers supersedes that request on a server that keeps
        // only the latest, and the initial paint is then never delivered. So
        // incremental requests wait for a rectangle (or FIRST_FRAME_DELAY), the
        // whole screen is asked for once at that point, and the cadence resumes
        // a moment later so that request is not superseded in turn.
        let mut request_gate_until = Instant::now() + FIRST_FRAME_DELAY;
        let mut released = false;
        let mut got_rect = false;

        'session: loop {
            if !running.load(Ordering::Relaxed) {
                return;
            }

            // Apply everything already queued, without blocking.
            let mut rects = 0u64;
            loop {
                match client.poll_event().await {
                    Ok(Some(event)) => {
                        if track(apply_event(event, &mut framebuffer, &mut rects), &mut pending) {
                            tracing::warn!(source = source_ref, "VNC session error, reconnecting");
                            break 'session;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!(source = source_ref, "VNC session ended: {e}");
                        break 'session;
                    }
                }
            }

            if rects > 0 {
                last_event_at = Instant::now();
                // A rectangle, not any event: the size announcement comes from
                // the handshake rather than from a request, so it says nothing
                // about whether the server is answering yet.
                got_rect = true;
            }

            if !released && (got_rect || Instant::now() >= request_gate_until) {
                released = true;
                request_gate_until = Instant::now() + FULL_FRAME_GRACE;
                tracing::debug!(source = source_ref, "VNC asking for the full frame");
                if let Err(e) = client.input(X11Event::FullRefresh).await {
                    tracing::warn!(source = source_ref, "VNC full refresh failed: {e}");
                    break 'session;
                }
            }
            let may_request = released && Instant::now() >= request_gate_until;

            // Nothing queued: wait for the next event, or for our own next timer.
            // Awaiting the channel instead of polling it lets the client refill
            // its two-slot output channel, which is what gates the decoder.
            if rects == 0 {
                let quiet_before = last_event_at.elapsed();
                let until_request = if may_request {
                    request_interval(quiet_before).saturating_sub(last_request_at.elapsed())
                } else {
                    request_gate_until.saturating_duration_since(Instant::now())
                };
                let until_publish = if pending {
                    PUBLISH_INTERVAL.saturating_sub(last_publish_at.elapsed())
                } else {
                    MAX_WAIT
                };
                let wait = next_wait(until_request, until_publish);
                tokio::select! {
                    event = client.recv_event() => match event {
                        Ok(event) => {
                            if track(apply_event(event, &mut framebuffer, &mut rects), &mut pending) {
                                tracing::warn!(source = source_ref, "VNC session error, reconnecting");
                                break 'session;
                            }
                            if rects > 0 {
                                last_event_at = Instant::now();
                                got_rect = true;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(source = source_ref, "VNC session ended: {e}");
                            break 'session;
                        }
                    },
                    _ = tokio::time::sleep(wait) => {}
                }
            }

            let quiet = last_event_at.elapsed();

            if pending
                && publish_due(SETTLE, quiet, last_publish_at.elapsed())
                && let Some(fb) = &framebuffer
            {
                let frame_span = tracing::debug_span!("vnc_frame");
                let _frame_guard = frame_span.entered();
                let started = Instant::now();
                let mut data = pool.take(fb.byte_len());
                fb.snapshot_into(&mut data);
                let copy_ms = started.elapsed().as_secs_f32() * 1000.0;
                let (w, h) = (fb.width(), fb.height());
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
                pending = false;
                last_publish_at = Instant::now();
            }

            if may_request && request_due(quiet, last_request_at.elapsed()) {
                if let Err(e) = client.input(X11Event::Refresh).await {
                    tracing::warn!(source = source_ref, "VNC refresh failed: {e}");
                    break 'session;
                }
                last_request_at = Instant::now();
            }

            // The keep-awake moves the remote pointer on its own timer, whether
            // or not the stream is busy: a request is not input, so only a real
            // pointer event stops the server blanking an idle session. The two
            // alternating positions make every event a genuine move.
            if config.keep_awake && last_wake_at.elapsed() >= WAKE_INTERVAL {
                let (x, y) = WAKE_POSITIONS[wake_index];
                tracing::debug!(source = source_ref, "VNC keep-awake: pointer to ({x}, {y})");
                if let Err(e) = client.input(X11Event::PointerEvent((x, y, 0).into())).await {
                    tracing::warn!(source = source_ref, "VNC keep-awake failed: {e}");
                    break 'session;
                }
                wake_index = (wake_index + 1) % WAKE_POSITIONS.len();
                last_wake_at = Instant::now();
            }
        }
    }
}

/// Open and authenticate one session. Returns `None` on any failure so the
/// caller retries after a delay.
async fn connect(
    host: &str,
    port: u16,
    label: &str,
    config: &VncSourceConfig,
    source_ref: &str,
) -> Option<VncClient> {
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            tracing::warn!(source = source_ref, "VNC connect to {label} failed: {e}");
            return None;
        }
        Err(_) => {
            tracing::warn!(source = source_ref, "VNC connect to {label} timed out");
            return None;
        }
    };

    let password = config.password.clone();
    let built = VncConnector::new(stream)
        .set_auth_method(async move { Ok::<String, vnc::VncError>(password) })
        .set_pixel_format(VncPixelFormat::bgra())
        // Lossless encodings only: Tight's JPEG rectangles would need a decoder.
        .add_encoding(VncEncoding::Zrle)
        .add_encoding(VncEncoding::CopyRect)
        .add_encoding(VncEncoding::Raw)
        .add_encoding(VncEncoding::DesktopSizePseudo)
        .add_encoding(VncEncoding::ExtendedDesktopSizePseudo)
        .add_encoding(VncEncoding::LastRectPseudo)
        // A viewer never takes an exclusive session: that would disconnect
        // whoever is actually using the machine.
        .allow_shared(true)
        .build();
    let state = match built {
        Ok(state) => state,
        Err(e) => {
            tracing::warn!(source = source_ref, "VNC handshake setup failed: {e}");
            return None;
        }
    };
    // Bounded, so a peer that stops answering cannot park the source thread.
    let state = match tokio::time::timeout(HANDSHAKE_TIMEOUT, state.try_start()).await.ok() {
        Some(Ok(state)) => state,
        Some(Err(e)) => {
            tracing::warn!(source = source_ref, "VNC handshake failed: {e}");
            return None;
        }
        None => {
            tracing::warn!(source = source_ref, "VNC handshake with {label} timed out");
            return None;
        }
    };
    return match state.finish() {
        Ok(client) => Some(client),
        Err(e) => {
            tracing::warn!(source = source_ref, "VNC session start failed: {e}");
            None
        }
    };
}

enum Applied {
    None,
    Dirty,
    Fatal,
}

/// Fold one applied event into the session state; `true` means stop the session.
fn track(result: Applied, pending: &mut bool) -> bool {
    return match result {
        Applied::None => false,
        Applied::Dirty => {
            *pending = true;
            false
        }
        Applied::Fatal => true,
    };
}

/// Apply one server event to the framebuffer; `Dirty` means a frame should be
/// published.
fn apply_event(event: VncEvent, framebuffer: &mut Option<Framebuffer>, rects: &mut u64) -> Applied {
    return match event {
        VncEvent::SetResolution(screen) => {
            resize(framebuffer, screen.width as u32, screen.height as u32)
        }
        VncEvent::DesktopUpdate(update) => match &update.layout {
            Some(layout) => resize(framebuffer, layout.width as u32, layout.height as u32),
            None => Applied::None,
        },
        VncEvent::RawImage(rect, data) => match framebuffer.as_mut() {
            Some(fb) => {
                fb.blit(
                    rect.x as u32,
                    rect.y as u32,
                    rect.width as u32,
                    rect.height as u32,
                    &data,
                );
                *rects += 1;
                Applied::Dirty
            }
            None => Applied::None,
        },
        VncEvent::Copy(dst, src) => match framebuffer.as_mut() {
            Some(fb) => {
                let (width, height) = (dst.width.min(src.width) as u32, dst.height.min(src.height) as u32);
                fb.copy_rect(
                    dst.x as u32,
                    dst.y as u32,
                    src.x as u32,
                    src.y as u32,
                    width,
                    height,
                );
                *rects += 1;
                Applied::Dirty
            }
            None => Applied::None,
        },
        VncEvent::Error(message) => {
            tracing::warn!("VNC error: {message}");
            Applied::Fatal
        }
        // SetPixelFormat, SetCursor, JpegImage, Bell and Text carry nothing the
        // tile renders.
        _ => Applied::None,
    };
}

fn resize(framebuffer: &mut Option<Framebuffer>, width: u32, height: u32) -> Applied {
    if width == 0 || height == 0 {
        return Applied::None;
    }
    match framebuffer {
        Some(fb) if fb.width() == width && fb.height() == height => Applied::None,
        Some(fb) => {
            fb.resize(width, height);
            Applied::Dirty
        }
        None => {
            *framebuffer = Some(Framebuffer::new(width, height));
            Applied::Dirty
        }
    }
}

impl Drop for VncSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take()
            && let Err(e) = t.join()
        {
            tracing::error!(source = self.source_ref, "Thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for VncSource {
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
    use std::io::{Read, Write};

    #[test]
    fn is_configured_requires_a_host() {
        assert!(!VncSourceConfig::default().is_configured());
        assert!(!VncSourceConfig { host: "   ".into(), ..Default::default() }.is_configured());
        assert!(VncSourceConfig { host: "studio".into(), ..Default::default() }.is_configured());
    }

    #[test]
    fn label_prefers_host_and_port() {
        let source_ref = "VNC 1";
        assert_eq!(VncSourceConfig::default().label(source_ref), "VNC 1");
        let configured = VncSourceConfig {
            host: " studio ".into(),
            port: 5901,
            ..Default::default()
        };
        assert_eq!(configured.label(source_ref), "studio:5901");
    }

    #[test]
    fn port_defaults_to_the_rfb_port() {
        assert_eq!(VncSourceConfig::default().port, 5900);
        let parsed: VncSourceConfig = serde_json::from_str(r#"{"host":"studio"}"#).unwrap();
        assert_eq!(parsed.port, 5900);
        assert_eq!(parsed.host, "studio");
    }

    #[test]
    fn resize_marks_dirty_only_on_a_change() {
        let mut framebuffer = None;
        assert!(matches!(resize(&mut framebuffer, 640, 480), Applied::Dirty));
        assert!(matches!(resize(&mut framebuffer, 640, 480), Applied::None));
        assert!(matches!(resize(&mut framebuffer, 0, 480), Applied::None));
        assert!(matches!(resize(&mut framebuffer, 800, 600), Applied::Dirty));
        assert_eq!(framebuffer.as_ref().unwrap().width(), 800);
    }

    /// While the stream is active requests are paced by the short interval, so
    /// the server is asked often enough not to become the sweep rate; once it
    /// goes quiet the interval relaxes.
    #[test]
    fn request_interval_follows_activity() {
        assert!(!request_due(Duration::ZERO, ACTIVE_REQUEST_INTERVAL / 2));
        assert!(request_due(Duration::ZERO, ACTIVE_REQUEST_INTERVAL));
        assert!(request_due(ACTIVE_WINDOW / 2, ACTIVE_REQUEST_INTERVAL));

        // Quiet: the floor relaxes to the idle interval.
        assert!(!request_due(ACTIVE_WINDOW, ACTIVE_REQUEST_INTERVAL));
        assert!(!request_due(ACTIVE_WINDOW, IDLE_REQUEST_INTERVAL / 2));
        assert!(request_due(ACTIVE_WINDOW, IDLE_REQUEST_INTERVAL));
    }

    /// The wait is the soonest due timer, bounded so it neither spins nor
    /// oversleeps past how long a stop may take to be noticed.
    #[test]
    fn wait_is_bounded_by_the_soonest_timer() {
        let soonest = next_wait(Duration::from_millis(12), Duration::from_millis(3));
        assert_eq!(soonest, Duration::from_millis(3));

        assert_eq!(next_wait(Duration::ZERO, Duration::ZERO), MIN_WAIT);
        let long = Duration::from_secs(30);
        assert_eq!(next_wait(long, long), MAX_WAIT);
    }

    /// The keep-awake alternates between two adjacent spots, so consecutive
    /// wakes are real moves rather than no-ops the server may drop.
    #[test]
    fn wake_positions_alternate_by_a_pixel() {
        assert_eq!(WAKE_POSITIONS.len(), 2);
        assert_ne!(WAKE_POSITIONS[0], WAKE_POSITIONS[1]);
    }

    /// 32 bpp, 24 depth, little-endian BGRA, matching `VncPixelFormat::bgra()`.
    const SERVER_PIXEL_FORMAT: [u8; 16] = [
        32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0,
    ];

    /// Drive one real RFB session: version exchange, no-auth handshake, a 2x2
    /// desktop painted by a single Raw rectangle, then hang up.
    fn serve_one_frame(listener: std::net::TcpListener) {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"RFB 003.008\n").unwrap();

        let mut version = [0u8; 12];
        stream.read_exact(&mut version).unwrap();

        stream.write_all(&[1, 1]).unwrap(); // one security type: None
        let mut chosen = [0u8; 1];
        stream.read_exact(&mut chosen).unwrap();
        stream.write_all(&0u32.to_be_bytes()).unwrap(); // SecurityResult: OK

        // ClientInit: the client sends its shared flag before ServerInit.
        let mut shared = [0u8; 1];
        stream.read_exact(&mut shared).unwrap();

        let mut init = Vec::new();
        init.extend_from_slice(&2u16.to_be_bytes()); // width
        init.extend_from_slice(&2u16.to_be_bytes()); // height
        init.extend_from_slice(&SERVER_PIXEL_FORMAT);
        init.extend_from_slice(&0u32.to_be_bytes()); // desktop name length
        stream.write_all(&init).unwrap();

        // The client sends SetPixelFormat, SetEncodings, then the first
        // FramebufferUpdateRequest that asks us to paint the screen.
        loop {
            let mut kind = [0u8; 1];
            stream.read_exact(&mut kind).unwrap();
            match kind[0] {
                0 => {
                    let mut rest = [0u8; 19];
                    stream.read_exact(&mut rest).unwrap();
                }
                2 => {
                    let mut rest = [0u8; 3];
                    stream.read_exact(&mut rest).unwrap();
                    let count = u16::from_be_bytes([rest[1], rest[2]]) as usize;
                    let mut encodings = vec![0u8; count * 4];
                    stream.read_exact(&mut encodings).unwrap();
                }
                3 => {
                    let mut request = [0u8; 9];
                    stream.read_exact(&mut request).unwrap();
                    let pixels: [u8; 16] = [
                        0x11, 0x22, 0x33, 0xff, 0x44, 0x55, 0x66, 0xff, 0x77, 0x88, 0x99, 0xff,
                        0xaa, 0xbb, 0xcc, 0xff,
                    ];
                    let mut update = vec![0u8, 0u8];
                    update.extend_from_slice(&1u16.to_be_bytes()); // one rectangle
                    update.extend_from_slice(&0u16.to_be_bytes()); // x
                    update.extend_from_slice(&0u16.to_be_bytes()); // y
                    update.extend_from_slice(&2u16.to_be_bytes()); // width
                    update.extend_from_slice(&2u16.to_be_bytes()); // height
                    update.extend_from_slice(&0i32.to_be_bytes()); // encoding: Raw
                    update.extend_from_slice(&pixels);
                    stream.write_all(&update).unwrap();
                    std::thread::sleep(Duration::from_millis(200));
                    return;
                }
                _ => return,
            }
        }
    }

    /// End to end: TCP connect, RFB handshake, encoding negotiation, Raw decode,
    /// framebuffer blit and publication.
    #[test]
    fn publishes_a_frame_from_a_real_rfb_session() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || serve_one_frame(listener));

        let config = VncSourceConfig {
            host: "127.0.0.1".to_string(),
            port,
            password: String::new(),
            keep_awake: false,
        };
        let source = VncSource::spawn("VNC 1".to_string(), &config);
        let deadline = Instant::now() + Duration::from_secs(10);
        let frame = loop {
            if let Some(frame) = source.slot.lock().unwrap().clone() {
                break Some(frame);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        let Some(Frame::Cpu(frame)) = frame else {
            panic!("no frame published by the VNC source");
        };
        assert_eq!((frame.w, frame.h), (2, 2));
        assert_eq!(frame.fmt, PixelFormat::Bgra8);
        assert_eq!(&frame.data[0..4], &[0x11, 0x22, 0x33, 0xff]);
        assert_eq!(&frame.data[12..16], &[0xaa, 0xbb, 0xcc, 0xff]);
    }
}
