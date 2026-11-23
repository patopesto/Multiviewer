use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::artnet;
use super::mtc;
use super::{Timecode, TimecodeDisplay, TimecodeState, TimecodeStatus};
use super::super::{CpuFrame, Frame, PixelFormat, SourceRef, SourceStats, TextStyle, VideoSource};

/// How long a received value is reported as live before it is called lost.
const STALE: Duration = Duration::from_secs(3);

/// The last value a receiver thread published. The value is held when the
/// signal stops; only the status changes.
#[derive(Default)]
pub(super) struct ReceiverState {
    timecode: Option<Timecode>,
    received_at: Option<Instant>,
    error: Option<String>,
}

impl ReceiverState {
    pub(super) fn fail(&mut self, error: String) {
        self.timecode = None;
        self.received_at = None;
        self.error = Some(error);
    }

    pub(super) fn clear_error(&mut self) {
        self.error = None;
    }

    pub(super) fn publish(&mut self, timecode: Timecode) {
        self.error = None;
        self.timecode = Some(timecode);
        self.received_at = Some(Instant::now());
    }

    pub(super) fn status(&self) -> TimecodeStatus {
        return TimecodeStatus {
            timecode: self.timecode,
            state: self.state(),
        };
    }

    fn state(&self) -> TimecodeState {
        if let Some(error) = &self.error {
            return TimecodeState::Error(error.clone());
        }
        let Some(received_at) = self.received_at else {
            return TimecodeState::Waiting;
        };
        return if received_at.elapsed() <= STALE {
            TimecodeState::Receiving
        } else {
            TimecodeState::Lost
        };
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// @docs protocol="Timecode" direction=input
pub struct TimecodeSourceConfig {
    /// Timecode input and its connection settings.
    pub input: TimecodeProtocol,
    /// Which fields of the received timecode are shown.
    pub display: TimecodeDisplay,
    /// Frame size, colours and alignment.
    #[serde(flatten)]
    pub style: TextStyle,
}

impl TimecodeSourceConfig {
    /// Largest card raster. The card re-renders on every new value, so a 4K
    /// frame would cost tens of MiB of memory traffic per change.
    pub const MAX_SIZE: (u32, u32) = (1920, 1080);
}

impl Default for TimecodeSourceConfig {
    fn default() -> Self {
        return Self {
            input: TimecodeProtocol::default(),
            display: TimecodeDisplay::default(),
            style: TextStyle::default(),
        };
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "protocol")]
pub enum TimecodeProtocol {
    ArtNet { bind: String, port: u16, stream_id: u8 },
    Mtc { port: String },
}

impl TimecodeProtocol {
    pub fn label(&self) -> &'static str {
        return match self {
            TimecodeProtocol::ArtNet { .. } => "Art-Net",
            TimecodeProtocol::Mtc { .. } => "MTC",
        };
    }

    /// Every known input with its default settings, for the settings picker.
    pub fn variants() -> Vec<Self> {
        return vec![
            Self::default(),
            Self::Mtc {
                port: String::new(),
            },
        ];
    }
}

impl Default for TimecodeProtocol {
    fn default() -> Self {
        return Self::ArtNet {
            bind: "0.0.0.0".to_string(),
            port: 6454,
            stream_id: 0,
        };
    }
}

struct Cached {
    value: Option<String>,
    frame: Frame,
    seq: u64,
}

/// A timecode display. The receiver thread publishes the last value, and the
/// frame is re-rendered only when the formatted value changes.
pub struct TimecodeSource {
    display: TimecodeDisplay,
    style: TextStyle,
    state: Arc<Mutex<ReceiverState>>,
    cache: Mutex<Cached>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TimecodeSource {
    pub fn spawn(source_ref: SourceRef, config: &TimecodeSourceConfig) -> Self {
        let display = config.display;
        let (max_width, max_height) = TimecodeSourceConfig::MAX_SIZE;
        let mut style = config.style.clone().bounded();
        style.width = style.width.min(max_width);
        style.height = style.height.min(max_height);
        let state = Arc::new(Mutex::new(ReceiverState::default()));
        let running = Arc::new(AtomicBool::new(true));
        let thread = start_receiver(&source_ref, &config.input, state.clone(), running.clone());

        let frame = text_frame(&style, display.placeholder(), 0);
        let mut stats = SourceStats::new();
        stats.record_frame(style.width, style.height, PixelFormat::Rgba8.label(), 0.0);
        return Self {
            display,
            style,
            state,
            cache: Mutex::new(Cached { value: None, frame, seq: 0 }),
            stats: Arc::new(Mutex::new(stats)),
            running,
            thread: Some(thread),
        };
    }
}

/// The thread owns the receiver for the source's whole life; the source only
/// flips `running` and joins it.
fn start_receiver(
    source_ref: &str,
    input: &TimecodeProtocol,
    state: Arc<Mutex<ReceiverState>>,
    running: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    return match input {
        TimecodeProtocol::ArtNet { bind, port, stream_id } => {
            let source = source_ref.to_string();
            let bind = bind.clone();
            let port = *port;
            let stream_id = *stream_id;
            std::thread::Builder::new()
                .name(format!("timecode-{source}"))
                .spawn(move || artnet::run(&source, &bind, port, stream_id, state, running))
                .expect("spawn timecode receiver")
        }
        TimecodeProtocol::Mtc { port } => {
            let source = source_ref.to_string();
            let port = port.clone();
            std::thread::Builder::new()
                .name(format!("timecode-{source}"))
                .spawn(move || mtc::run(&source, &port, state, running))
                .expect("spawn timecode receiver")
        }
    };
}

impl Drop for TimecodeSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl VideoSource for TimecodeSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        let status = self.timecode().unwrap_or_default();
        let value = status.timecode.map(|timecode| self.display.format(&timecode));
        let mut cache = self.cache.lock().unwrap();
        if cache.value != value {
            let text = value.as_deref().unwrap_or_else(|| self.display.placeholder());
            let frame = text_frame(&self.style, text, cache.seq + 1);
            cache.seq += 1;
            cache.frame = frame;
            cache.value = value;
            let mut stats = self.stats.lock().unwrap();
            stats.record_frame(
                self.style.width,
                self.style.height,
                PixelFormat::Rgba8.label(),
                status.timecode.map_or(0.0, |tc| tc.fps),
            );
        }
        return Some(cache.frame.clone());
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }

    fn alive(&self) -> bool {
        return self.thread.as_ref().is_none_or(|thread| !thread.is_finished());
    }

    fn timecode(&self) -> Option<TimecodeStatus> {
        return Some(self.state.lock().unwrap().status());
    }
}

fn text_frame(style: &TextStyle, text: &str, seq: u64) -> Frame {
    return Frame::Cpu(CpuFrame {
        data: Arc::new(style.render(text)),
        w: style.width,
        h: style.height,
        fmt: PixelFormat::Rgba8,
        pitch: 0,
        seq,
    });
}

#[cfg(test)]
mod tests {
    use super::super::common::test_timecode;
    use super::*;

    #[test]
    fn the_source_exposes_the_raw_timecode() {
        let config = TimecodeSourceConfig {
            input: TimecodeProtocol::ArtNet {
                bind: "127.0.0.1".to_string(),
                port: 0,
                stream_id: 0,
            },
            ..Default::default()
        };
        let source = TimecodeSource::spawn("Timecode 1".to_string(), &config);
        assert_eq!(
            source.timecode(),
            Some(TimecodeStatus {
                timecode: None,
                state: TimecodeState::Waiting,
            })
        );

        source.state.lock().unwrap().publish(test_timecode());
        assert_eq!(
            source.timecode(),
            Some(TimecodeStatus {
                timecode: Some(test_timecode()),
                state: TimecodeState::Receiving,
            })
        );
    }

    #[test]
    fn the_mtc_input_round_trips_through_a_project() {
        let config = TimecodeSourceConfig {
            input: TimecodeProtocol::Mtc {
                port: "IAC Driver Bus 1".to_string(),
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"protocol\":\"Mtc\""));
        assert_eq!(serde_json::from_str::<TimecodeSourceConfig>(&json).unwrap(), config);
    }

    #[test]
    fn an_mtc_input_ignores_a_stored_id() {
        let input: TimecodeProtocol =
            serde_json::from_str(r#"{"protocol":"Mtc","port":"IAC Driver Bus 1","id":"coremidi:1234"}"#).unwrap();
        assert_eq!(
            input,
            TimecodeProtocol::Mtc {
                port: "IAC Driver Bus 1".to_string(),
            }
        );
    }

    #[test]
    fn a_large_card_is_capped_to_a_full_hd_raster() {
        let config = TimecodeSourceConfig {
            input: TimecodeProtocol::ArtNet {
                bind: "127.0.0.1".to_string(),
                port: 0,
                stream_id: 0,
            },
            style: TextStyle {
                width: 4096,
                height: 4096,
                ..Default::default()
            },
            ..Default::default()
        };
        let source = TimecodeSource::spawn("Timecode 1".to_string(), &config);
        let cache = source.cache.lock().unwrap();
        let Frame::Cpu(frame) = &cache.frame else {
            unreachable!("timecode sources publish CPU frames");
        };
        assert_eq!((frame.w, frame.h), TimecodeSourceConfig::MAX_SIZE);
    }

    #[test]
    fn a_lost_signal_keeps_its_last_value() {
        let mut state = ReceiverState::default();
        assert_eq!(state.status(), TimecodeStatus::default());

        state.publish(test_timecode());
        assert_eq!(
            state.status(),
            TimecodeStatus {
                timecode: Some(test_timecode()),
                state: TimecodeState::Receiving,
            }
        );

        state.received_at = Some(Instant::now().checked_sub(STALE + Duration::from_secs(1)).unwrap());
        assert_eq!(
            state.status(),
            TimecodeStatus {
                timecode: Some(test_timecode()),
                state: TimecodeState::Lost,
            },
            "a lost signal keeps the last value"
        );
    }

    #[test]
    fn an_error_wins_over_the_last_value() {
        let mut state = ReceiverState::default();
        state.publish(test_timecode());
        state.fail("bind failed".to_string());
        assert_eq!(
            state.status(),
            TimecodeStatus {
                timecode: None,
                state: TimecodeState::Error("bind failed".to_string()),
            }
        );
    }
}
