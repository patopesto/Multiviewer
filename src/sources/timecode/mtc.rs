use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use midir::{MidiInput, MidiInputConnection};

use crate::sources::PollGate;

use crate::APP_NAME;
use super::source::ReceiverState;
use super::Timecode;

const CLIENT: &str = APP_NAME;
const CONNECTION: &str = format!("{CLIENT}-mtc");
const POLL: Duration = Duration::from_millis(200);
const RETRY: Duration = Duration::from_secs(1);
const DISCOVERY: Duration = Duration::from_secs(2);
/// Quarter frames describe the time at piece 0, two frames before piece 7.
const QUARTER_FRAME_ADVANCE: u8 = 2;

static GATE: OnceLock<PollGate> = OnceLock::new();

/// Wake the MTC discovery from a CoreMIDI device-change notification.
pub fn request_refresh() {
    if let Some(gate) = GATE.get() {
        gate.request();
    }
}

/// Background MIDI input discovery, like the other protocol discoveries.
pub struct MtcDiscovery {
    ports: Arc<Mutex<Vec<String>>>,
    gate: PollGate,
}

impl MtcDiscovery {
    pub fn start() -> Self {
        let ports = Arc::new(Mutex::new(Vec::new()));
        let cache = ports.clone();
        let gate = PollGate::with_interval(DISCOVERY);
        let _ = GATE.set(gate.clone());
        let wait = gate.clone();
        std::thread::Builder::new()
            .name("mtc-discovery".to_string())
            .spawn(move || {
                loop {
                    let next = scan_ports();
                    let mut current = cache.lock().unwrap();
                    if *current != next {
                        tracing::debug!(ports = next.len(), "MTC inputs changed");
                    }
                    *current = next;
                    drop(current);
                    wait.wait();
                }
            })
            .expect("spawn mtc-discovery");
        return Self { ports, gate };
    }

    pub fn list(&self) -> Vec<String> {
        return self.ports.lock().unwrap().clone();
    }

    pub fn request_refresh(&self) {
        self.gate.request();
    }
}

/// MIDI inputs this process can open, for the settings picker.
#[cfg(target_os = "macos")]
fn scan_ports() -> Vec<String> {
    let mut names: Vec<String> = coremidi::Sources.into_iter().filter_map(|source| source.display_name()).collect();
    names.sort();
    names.dedup();
    return names;
}

#[cfg(not(target_os = "macos"))]
fn scan_ports() -> Vec<String> {
    let Ok(input) = MidiInput::new(CLIENT) else {
        return Vec::new();
    };
    let mut names: Vec<String> = input.ports().into_iter().filter_map(|port| input.port_name(&port).ok()).collect();
    names.sort();
    names.dedup();
    return names;
}

/// Receive MTC until `running` clears, retrying a port that is not there yet.
pub(super) fn run(source: &str, port_name: &str, state: Arc<Mutex<ReceiverState>>, running: Arc<AtomicBool>) {
    if port_name.is_empty() {
        state.lock().unwrap().fail("no MIDI input selected".to_string());
        return;
    }
    let mut input = match MidiInput::new(CLIENT) {
        Ok(input) => input,
        Err(error) => {
            state.lock().unwrap().fail(format!("MIDI unavailable: {error}"));
            return;
        }
    };
    while running.load(Ordering::Relaxed) {
        match connect(input, source, port_name, &state) {
            Ok(connection) => {
                state.lock().unwrap().clear_error();
                while running.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL);
                }
                drop(connection);
                return;
            }
            Err((returned, error)) => {
                input = returned;
                state.lock().unwrap().fail(error);
                std::thread::sleep(RETRY);
            }
        }
    }
}

/// Match by name, not by backend id: a project moved to another computer must
/// find the same device, and ids are machine-local. Same-named inputs resolve
/// to the first one. Reuses the caller's client so a retry does not create a
/// new CoreMIDI client.
fn connect(
    input: MidiInput,
    source: &str,
    port_name: &str,
    state: &Arc<Mutex<ReceiverState>>,
) -> Result<MidiInputConnection<Receiver>, (MidiInput, String)> {
    let Some(port) = input.ports().into_iter().find(|port| input.port_name(port).is_ok_and(|name| name == port_name)) else {
        return Err((input, format!("MIDI input '{port_name}' not found")));
    };
    let receiver = Receiver {
        source: source.to_string(),
        state: state.clone(),
        parser: Parser::default(),
    };
    return match input.connect(
        &port,
        CONNECTION,
        |_timestamp, message, receiver: &mut Receiver| {
            let Some(timecode) = receiver.parser.push(message) else {
                return;
            };
            tracing::debug!(source = %receiver.source, timecode = %timecode.label(), "mtc timecode");
            receiver.state.lock().unwrap().publish(timecode);
        },
        receiver,
    ) {
        Ok(connection) => Ok(connection),
        Err(error) => {
            let message = format!("MIDI input '{port_name}' failed: {error}");
            return Err((error.into_inner(), message));
        }
    };
}

struct Receiver {
    source: String,
    state: Arc<Mutex<ReceiverState>>,
    parser: Parser,
}

/// Assembles MTC quarter frames, or reads a full-frame message. Pieces are
/// expected in order 0..7; a gap discards the set, so reverse playback holds
/// the last value. A sequence advances the value at its halfway frame boundary
/// and again when the last piece completes.
#[derive(Default)]
struct Parser {
    pieces: [u8; 8],
    seen: u8,
    expected: u8,
    /// The last current-time value, for the midpoint step.
    last: Option<Timecode>,
}

impl Parser {
    fn push(&mut self, message: &[u8]) -> Option<Timecode> {
        match message {
            [0xF1, data] => return self.quarter(*data),
            [0xF0, 0x7F, _device, 0x01, 0x01, hours, minutes, seconds, frames, 0xF7] => {
                self.reset();
                self.last = Timecode::from_rate(*hours & 0x1F, *minutes, *seconds, *frames, (*hours >> 5) & 0x03);
                return self.last;
            }
            _ => return None,
        }
    }

    /// A quarter frame carries `0rrh` hours in piece 7, with the rate in `rr`.
    fn quarter(&mut self, data: u8) -> Option<Timecode> {
        if data & 0x80 != 0 {
            return None;
        }
        let piece = data >> 4;
        if piece == 0 {
            if self.seen != 0 {
                self.last = None;
            }
            self.reset();
        } else if piece != self.expected {
            self.reset();
            self.last = None;
            return None;
        }
        self.pieces[piece as usize] = data & 0x0F;
        self.seen |= 1 << piece;
        self.expected = (piece + 1) & 0x07;
        // F1 4X marks the second frame boundary of the two-frame cycle.
        if piece == 4 && self.seen == 0x1F {
            let next = self.last?.add_frames(1);
            self.last = Some(next);
            return Some(next);
        }
        if self.seen != 0xFF {
            return None;
        }
        self.seen = 0;
        let rate = (self.pieces[7] >> 1) & 0x03;
        let hours = self.pieces[6] | ((self.pieces[7] & 0x01) << 4);
        let minutes = self.pieces[4] | ((self.pieces[5] & 0x03) << 4);
        let seconds = self.pieces[2] | ((self.pieces[3] & 0x03) << 4);
        let frames = self.pieces[0] | ((self.pieces[1] & 0x01) << 4);
        let Some(timecode) = Timecode::from_rate(hours, minutes, seconds, frames, rate) else {
            self.last = None;
            return None;
        };
        let advanced = timecode.add_frames(QUARTER_FRAME_ADVANCE);
        self.last = Some(advanced);
        return Some(advanced);
    }

    fn reset(&mut self) {
        self.pieces = [0; 8];
        self.seen = 0;
        self.expected = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quarter(parser: &mut Parser, piece: u8, value: u8) -> Option<Timecode> {
        return parser.push(&[0xF1, (piece << 4) | value]);
    }

    fn feed(parser: &mut Parser, pieces: [u8; 8]) -> Option<Timecode> {
        let mut published = None;
        for (piece, value) in pieces.into_iter().enumerate() {
            published = quarter(parser, piece as u8, value);
        }
        return published;
    }

    fn full_frame(device: u8, hours: u8, minutes: u8, seconds: u8, frames: u8) -> [u8; 10] {
        return [0xF0, 0x7F, device, 0x01, 0x01, hours, minutes, seconds, frames, 0xF7];
    }

    #[test]
    fn quarter_frames_assemble_the_sent_timecode() {
        let mut parser = Parser::default();
        // 01:02:03:04 at 25 fps: frames 4, seconds 3, minutes 2, hours 1, rate 1.
        let timecode = feed(&mut parser, [4, 0, 3, 0, 2, 0, 1, 1 << 1]).unwrap();
        assert_eq!(timecode.label(), "01:02:03:06", "piece 7 arrives two frames after piece 0");
        assert_eq!(timecode.fps, 25.0);
        assert!(!timecode.drop_frame);
    }

    #[test]
    fn quarter_frames_advance_across_a_minute() {
        let mut parser = Parser::default();
        // 00:00:59:29 at 30 fps: frames 29, seconds 59, rate 3.
        let timecode = feed(&mut parser, [13, 1, 11, 3, 0, 0, 0, 3 << 1]).unwrap();
        assert_eq!(timecode.label(), "00:01:00:01");
    }

    #[test]
    fn quarter_frames_advance_across_a_drop_frame_minute() {
        let mut parser = Parser::default();
        // 00:00:59;29 at 29.97 fps: frames 29, seconds 59, rate 2.
        let timecode = feed(&mut parser, [13, 1, 11, 3, 0, 0, 0, 2 << 1]).unwrap();
        assert_eq!(timecode.label(), "00:01:00;03");
    }

    #[test]
    fn the_midpoint_advances_the_last_value_by_one_frame() {
        let mut parser = Parser::default();
        let first = feed(&mut parser, [4, 0, 3, 0, 2, 0, 1, 1 << 1]).unwrap();
        assert_eq!(first.label(), "01:02:03:06");

        // The next cycle encodes the same fields two frames later.
        let pieces = [6, 0, 3, 0, 2, 0, 1, 1 << 1];
        let mut published = None;
        for piece in 0..4 {
            published = quarter(&mut parser, piece, pieces[piece as usize]);
        }
        assert!(published.is_none(), "pieces before F1 4X do not publish");
        assert_eq!(quarter(&mut parser, 4, pieces[4]).unwrap().label(), "01:02:03:07", "F1 4X advances one frame");

        for piece in 5..8 {
            published = quarter(&mut parser, piece, pieces[piece as usize]);
        }
        assert_eq!(published.unwrap().label(), "01:02:03:08", "the completed cycle adds the second frame");
    }

    #[test]
    fn a_gap_skips_the_midpoint() {
        let pieces = [4, 0, 3, 0, 2, 0, 1, 1 << 1];
        let mut parser = Parser::default();
        assert!(feed(&mut parser, pieces).is_some());

        let mut published = None;
        for piece in [0u8, 1, 3, 4] {
            published = quarter(&mut parser, piece, pieces[piece as usize]);
        }
        assert!(published.is_none(), "a missing piece discards the midpoint");
        assert!(feed(&mut parser, pieces).is_some(), "the next complete cycle publishes");
    }

    #[test]
    fn an_unfinished_sequence_breaks_the_midpoint_chain() {
        let pieces = [4, 0, 3, 0, 2, 0, 1, 1 << 1];
        let mut parser = Parser::default();
        assert!(feed(&mut parser, pieces).is_some());

        for piece in 0..7 {
            quarter(&mut parser, piece, pieces[piece as usize]);
        }
        let mut published = None;
        for piece in 0..4 {
            published = quarter(&mut parser, piece, pieces[piece as usize]);
        }
        assert!(published.is_none(), "the next midpoint does not use the unfinished sequence");
        assert!(quarter(&mut parser, 4, pieces[4]).is_none(), "the midpoint is skipped");
        assert!(feed(&mut parser, pieces).is_some(), "a complete sequence publishes again");
    }

    #[test]
    fn a_full_frame_sets_the_midpoint_base() {
        let mut parser = Parser::default();
        let full = parser.push(&full_frame(0x7F, (3 << 5) | 1, 2, 3, 4)).unwrap();
        assert_eq!(full.label(), "01:02:03:04");

        let pieces = [6, 0, 3, 0, 2, 0, 1, 3 << 1];
        let mut published = None;
        for piece in 0..4 {
            published = quarter(&mut parser, piece, pieces[piece as usize]);
        }
        assert!(published.is_none());
        assert_eq!(quarter(&mut parser, 4, pieces[4]).unwrap().label(), "01:02:03:05");
    }

    #[test]
    fn rates_come_from_the_last_quarter_frame() {
        for (code, fps, drop_frame) in [(0, 24.0, false), (1, 25.0, false), (2, 30_000.0 / 1001.0, true), (3, 30.0, false)] {
            let mut parser = Parser::default();
            let timecode = feed(&mut parser, [0, 0, 0, 0, 0, 0, 0, code << 1]).unwrap();
            assert_eq!(timecode.fps, fps, "rate code {code}");
            assert_eq!(timecode.drop_frame, drop_frame, "rate code {code}");
        }
    }

    #[test]
    fn a_full_frame_sysex_publishes_immediately() {
        let mut parser = Parser::default();
        let timecode = parser.push(&full_frame(0x7F, (3 << 5) | 23, 59, 58, 29)).unwrap();
        assert_eq!(timecode.label(), "23:59:58:29");
        assert_eq!(timecode.fps, 30.0);
        assert!(!timecode.drop_frame);

        let drop_frame = parser.push(&full_frame(0x01, (2 << 5) | 12, 1, 2, 3)).unwrap();
        assert_eq!(drop_frame.label(), "12:01:02;03");
        assert!((drop_frame.fps - 29.97).abs() < 1e-2);
        assert!(drop_frame.drop_frame);
    }

    #[test]
    fn a_missing_piece_waits_for_the_next_cycle() {
        let pieces = [4, 0, 3, 0, 2, 0, 1, 2];
        let mut parser = Parser::default();
        let mut published = None;
        for piece in [0, 1, 2, 3, 5, 6, 7] {
            published = quarter(&mut parser, piece, pieces[piece as usize]);
        }
        assert!(published.is_none(), "a missing piece publishes nothing");
        assert!(feed(&mut parser, pieces).is_some(), "the next complete set publishes");
    }

    #[test]
    fn other_midi_messages_are_ignored() {
        let mut parser = Parser::default();
        for message in [
            &[0x90][..],
            &[0x90, 60, 100],
            &[0xF8],
            &[0xF1],
            &[0xF1, 0x00, 0x00],
            &[0xF1, 0x80],
            &[0xF1, 0xFF],
            &[0xF0, 0x7F, 0x7F, 0x01, 0x02, 0, 0, 0, 0, 0xF7],
            &[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0, 0, 0, 0],
        ] {
            assert!(parser.push(message).is_none(), "{message:x?}");
        }
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut parser = Parser::default();
        assert!(parser.push(&full_frame(0x7F, 24, 0, 0, 0)).is_none(), "24 hours");
        assert!(parser.push(&full_frame(0x7F, 0, 60, 0, 0)).is_none(), "60 minutes");
        assert!(parser.push(&full_frame(0x7F, 0, 0, 60, 0)).is_none(), "60 seconds");
        assert!(parser.push(&full_frame(0x7F, 1 << 5, 0, 0, 25)).is_none(), "25 frames at 25 fps");
        assert!(parser.push(&full_frame(0x7F, 0, 0, 0, 24)).is_none(), "24 frames at 24 fps");
        assert!(parser.push(&full_frame(0x7F, (3 << 5) | 23, 59, 59, 29)).is_some(), "the last valid frame");
    }
}
