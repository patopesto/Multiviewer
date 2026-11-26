use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::artnet;
use super::ltc;
use super::mtc;
use super::{Timecode, TimecodeState, TimecodeStatus};

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

    /// When the last value arrived, if one ever did.
    pub(super) fn received_at(&self) -> Option<Instant> {
        return self.received_at;
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

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "protocol")]
pub enum TimecodeProtocol {
    ArtNet { bind: String, port: u16, stream_id: u8 },
    Mtc { port: String },
    Ltc { device: String, channel: u16 },
}

impl TimecodeProtocol {
    pub fn label(&self) -> &'static str {
        return match self {
            TimecodeProtocol::ArtNet { .. } => "Art-Net",
            TimecodeProtocol::Mtc { .. } => "MTC",
            TimecodeProtocol::Ltc { .. } => "LTC",
        };
    }

    /// Every known input with its default settings, for the settings picker.
    pub fn variants() -> Vec<Self> {
        return vec![
            Self::default(),
            Self::Mtc { port: String::new() },
            Self::Ltc {
                device: String::new(),
                channel: 1,
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

/// One receiver thread per input, shared by every card that displays it, so an
/// Art-Net socket, MIDI port or audio device is opened once however many
/// placements show it.
pub(super) struct SharedInput {
    key: TimecodeProtocol,
    state: Arc<Mutex<ReceiverState>>,
    running: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    cards: AtomicUsize,
}

impl SharedInput {
    fn start(input: &TimecodeProtocol) -> Self {
        let state = Arc::new(Mutex::new(ReceiverState::default()));
        let running = Arc::new(AtomicBool::new(true));
        let thread = start_receiver(input, state.clone(), running.clone());
        return Self {
            key: input.clone(),
            state,
            running,
            thread: Mutex::new(Some(thread)),
            cards: AtomicUsize::new(1),
        };
    }

    pub(super) fn status(&self) -> TimecodeStatus {
        return self.state.lock().unwrap().status();
    }

    /// The input this receiver is bound to.
    pub(super) fn key(&self) -> &TimecodeProtocol {
        return &self.key;
    }

    #[cfg(test)]
    pub(super) fn state(&self) -> &Arc<Mutex<ReceiverState>> {
        return &self.state;
    }

    /// Whether the receiver is still running. A backend that fails on open
    /// finishes its thread instead of reporting through the state.
    pub(super) fn alive(&self) -> bool {
        return self.thread.lock().unwrap().as_ref().is_some_and(|thread| !thread.is_finished());
    }

    /// Restart a receiver that gave up, so a later card on the same input gets
    /// a fresh attempt instead of inheriting the dead one.
    fn reopen(&self) {
        let mut thread = self.thread.lock().unwrap();
        if thread.as_ref().is_some_and(|thread| !thread.is_finished()) {
            return;
        }
        self.running.store(true, Ordering::Relaxed);
        // The failed backend left its error in the state; the new attempt has to
        // clear it, because not every backend does so after a successful open.
        self.state.lock().unwrap().clear_error();
        *thread = Some(start_receiver(&self.key, self.state.clone(), self.running.clone()));
    }

    fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }
}

/// Every input currently in use. Cards attach when they open and detach when
/// they drop; the last one to leave stops the receiver.
#[derive(Default)]
pub struct TimecodeHub {
    inputs: Mutex<HashMap<TimecodeProtocol, Arc<SharedInput>>>,
}

impl std::fmt::Debug for TimecodeHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return f.write_str("TimecodeHub");
    }
}

impl TimecodeHub {
    pub(super) fn attach(&self, input: &TimecodeProtocol) -> Arc<SharedInput> {
        let mut inputs = self.inputs.lock().unwrap();
        if let Some(shared) = inputs.get(input) {
            shared.cards.fetch_add(1, Ordering::AcqRel);
            shared.reopen();
            return shared.clone();
        }
        let shared = Arc::new(SharedInput::start(input));
        inputs.insert(input.clone(), shared.clone());
        return shared;
    }

    pub(super) fn detach(&self, shared: &Arc<SharedInput>) {
        let mut inputs = self.inputs.lock().unwrap();
        let Some(entry) = inputs.get(&shared.key) else {
            return;
        };
        if !Arc::ptr_eq(entry, shared) {
            return;
        }
        if shared.cards.fetch_sub(1, Ordering::AcqRel) > 1 {
            return;
        }
        let stopped = inputs.remove(&shared.key);
        drop(inputs);
        if let Some(stopped) = stopped {
            stopped.stop();
        }
    }
}

/// What a receiver is reading, for its thread name and log lines. Several cards
/// can share one receiver, so a card's name would point at the wrong one.
fn label(input: &TimecodeProtocol) -> String {
    return match input {
        TimecodeProtocol::ArtNet { bind, port, stream_id } => format!("Art-Net {bind}:{port}/{stream_id}"),
        TimecodeProtocol::Mtc { port } => format!("MTC {port}"),
        TimecodeProtocol::Ltc { device, channel } => format!("LTC {device} channel {channel}"),
    };
}

fn start_receiver(input: &TimecodeProtocol, state: Arc<Mutex<ReceiverState>>, running: Arc<AtomicBool>) -> JoinHandle<()> {
    let label = label(input);
    return match input {
        TimecodeProtocol::ArtNet { bind, port, stream_id } => {
            let bind = bind.clone();
            let port = *port;
            let stream_id = *stream_id;
            std::thread::Builder::new()
                .name(format!("timecode-{label}"))
                .spawn(move || artnet::run(&label, &bind, port, stream_id, state, running))
                .expect("spawn timecode receiver")
        }
        TimecodeProtocol::Mtc { port } => {
            let port = port.clone();
            std::thread::Builder::new()
                .name(format!("timecode-{label}"))
                .spawn(move || mtc::run(&label, &port, state, running))
                .expect("spawn timecode receiver")
        }
        TimecodeProtocol::Ltc { device, channel } => {
            let device = device.clone();
            let channel = *channel;
            std::thread::Builder::new()
                .name(format!("timecode-{label}"))
                .spawn(move || ltc::run(&label, &device, channel, state, running))
                .expect("spawn timecode receiver")
        }
    };
}

#[cfg(test)]
mod tests {
    use crate::sources::TimecodeState;

    use super::super::common::test_timecode;
    use super::*;

    fn artnet() -> TimecodeProtocol {
        return TimecodeProtocol::ArtNet {
            bind: "127.0.0.1".to_string(),
            port: 0,
            stream_id: 0,
        };
    }

    fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        return false;
    }

    #[test]
    fn cards_on_one_input_share_a_receiver() {
        let hub = TimecodeHub::default();
        let input = artnet();
        let first = hub.attach(&input);
        let second = hub.attach(&input);
        assert!(Arc::ptr_eq(&first, &second));

        hub.detach(&first);
        let third = hub.attach(&input);
        assert!(Arc::ptr_eq(&third, &first));

        hub.detach(&second);
        hub.detach(&third);
        let fresh = hub.attach(&input);
        assert!(!Arc::ptr_eq(&fresh, &first));
        hub.detach(&fresh);
    }

    #[test]
    fn the_last_card_stops_the_receiver() {
        let hub = TimecodeHub::default();
        let input = artnet();
        let shared = hub.attach(&input);
        assert!(shared.alive());
        hub.detach(&shared);
        assert!(!shared.alive());
    }

    #[test]
    fn a_dead_receiver_reopens_when_a_card_attaches_again() {
        let hub = TimecodeHub::default();
        let holder = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let port = holder.local_addr().unwrap().port();
        let input = TimecodeProtocol::ArtNet {
            bind: "127.0.0.1".to_string(),
            port,
            stream_id: 0,
        };

        let first = hub.attach(&input);
        assert!(wait_until(|| !first.alive()), "the receiver gives up on a taken port");

        drop(holder);
        let second = hub.attach(&input);
        assert!(Arc::ptr_eq(&first, &second), "the entry is reopened in place");

        let sender = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let packet = artnet::packet(0, 1, 2, 3, 4, 0);
        assert!(
            wait_until(|| {
                let _ = sender.send_to(&packet, ("127.0.0.1", port));
                second.status().timecode.is_some()
            }),
            "the reopened receiver accepts packets"
        );
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
