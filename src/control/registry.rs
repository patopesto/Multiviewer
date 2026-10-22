use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use super::command::Command;
use super::config::ControlConfig;
use super::osc::{FeedbackMode, FeedbackValue, OscServer, normalize_prefix};

/// Messages applied per frame; a UDP flood must not stall a render frame.
const MAX_COMMANDS_PER_FRAME: usize = 200;

/// A controller that has not sent anything for this long stops receiving feedback.
const FEEDBACK_TTL: Duration = Duration::from_secs(600);

/// Upper bound on reply-to-sender feedback targets.
const MAX_FEEDBACK_SENDERS: usize = 16;

#[derive(Clone, Debug, Default)]
pub enum ControlStatus {
    #[default]
    Stopped,
    Listening(SocketAddr),
    Error(String),
}

/// What a control listener hands to the UI thread.
pub enum ControlMessage {
    Command(Command),
    /// A datagram arrived from this address; it can become a feedback target.
    Seen(SocketAddr),
}

/// The live control listeners. Owned by the engine on the UI thread; each
/// listener runs on its own thread and sends messages over a channel.
pub struct ControlRegistry {
    tx: Sender<ControlMessage>,
    rx: Receiver<ControlMessage>,
    osc: Option<OscServer>,
    status: ControlStatus,
    prefix: String,
    feedback_mode: FeedbackMode,
    feedback_target: Option<SocketAddr>,
    senders: HashMap<SocketAddr, Instant>,
    pending_full: HashSet<SocketAddr>,
    last_values: HashMap<String, FeedbackValue>,
}

impl ControlRegistry {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            osc: None,
            status: ControlStatus::Stopped,
            prefix: String::new(),
            feedback_mode: FeedbackMode::Off,
            feedback_target: None,
            senders: HashMap::new(),
            pending_full: HashSet::new(),
            last_values: HashMap::new(),
        }
    }

    /// Start, restart or stop the listeners to match `config`.
    pub fn apply(&mut self, config: &ControlConfig) {
        // Dropping the old server frees its port before the replacement binds.
        self.osc = None;
        self.status = ControlStatus::Stopped;
        self.prefix.clear();
        self.feedback_mode = FeedbackMode::Off;
        self.feedback_target = None;
        self.senders.clear();
        self.pending_full.clear();
        self.last_values.clear();

        let osc = &config.osc;
        if !osc.enabled {
            return;
        }
        let prefix = match normalize_prefix(&osc.prefix) {
            Ok(prefix) => prefix,
            Err(e) => {
                self.status = ControlStatus::Error(e.to_string());
                return;
            }
        };
        self.feedback_mode = osc.feedback_mode;
        if matches!(
            osc.feedback_mode,
            FeedbackMode::Configured | FeedbackMode::Both
        ) {
            let configured = !osc.feedback_host.trim().is_empty() && osc.feedback_port != 0;
            self.feedback_target = resolve_feedback_target(&osc.feedback_host, osc.feedback_port);
            if configured && self.feedback_target.is_none() {
                tracing::warn!(
                    "OSC feedback target {}:{} could not be resolved; feedback is disabled",
                    osc.feedback_host.trim(),
                    osc.feedback_port
                );
            }
        }
        match OscServer::start(&osc.bind, osc.port, prefix.clone(), self.tx.clone()) {
            Ok((server, addr)) => {
                self.osc = Some(server);
                self.prefix = prefix;
                self.status = ControlStatus::Listening(addr);
                if let Some(target) = self.feedback_target
                    && is_self_target(target, addr)
                {
                    tracing::warn!(
                        "OSC feedback target {target} points at the listener itself; feedback is disabled"
                    );
                    self.feedback_target = None;
                }
                tracing::info!("OSC control listening on {addr}");
            }
            Err(e) => {
                self.status = ControlStatus::Error(e.to_string());
                tracing::warn!("OSC control failed to start: {e}");
            }
        }
    }

    /// Take the commands received since the last call.
    pub fn drain(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        let mut handled = 0;
        while handled < MAX_COMMANDS_PER_FRAME {
            match self.rx.try_recv() {
                Ok(ControlMessage::Command(command)) => commands.push(command),
                Ok(ControlMessage::Seen(from)) => self.remember_sender(from),
                Err(_) => return commands,
            }
            handled += 1;
        }
        let mut dropped = 0;
        while let Ok(message) = self.rx.try_recv() {
            match message {
                ControlMessage::Command(_) => dropped += 1,
                ControlMessage::Seen(from) => self.remember_sender(from),
            }
        }
        if dropped > 0 {
            tracing::warn!("dropped {dropped} control commands (flood)");
        }
        return commands;
    }

    /// Send only what changed to the current feedback targets.
    pub fn publish_feedback(&mut self, values: Vec<(String, FeedbackValue)>) {
        let targets = self.feedback_targets();
        if !targets.is_empty() {
            let full = self.last_values.is_empty();
            let changed = diff(&self.last_values, &values);
            for target in targets {
                if full || self.pending_full.contains(&target) {
                    for (address, value) in &values {
                        self.send_feedback(target, address, value);
                    }
                } else {
                    for (address, value) in &changed {
                        self.send_feedback(target, address, value);
                    }
                }
            }
        }
        self.pending_full.clear();
        self.last_values = values.into_iter().collect();
    }

    pub fn status(&self) -> &ControlStatus {
        return &self.status;
    }

    pub fn feedback_mode(&self) -> FeedbackMode {
        return self.feedback_mode;
    }

    pub fn feedback_target(&self) -> Option<SocketAddr> {
        return self.feedback_target;
    }

    pub fn is_enabled(&self) -> bool {
        return self.osc.is_some();
    }

    /// Whether feedback values are worth building this frame.
    pub fn feedback_enabled(&self) -> bool {
        if self.osc.is_none() {
            return false;
        }
        return match self.feedback_mode {
            FeedbackMode::Off => false,
            FeedbackMode::Senders | FeedbackMode::Both => true,
            FeedbackMode::Configured => self.feedback_target.is_some(),
        };
    }

    fn send_feedback(&self, target: SocketAddr, address: &str, value: &FeedbackValue) {
        if let Some(osc) = &self.osc {
            osc.send_to(target, address, value);
        }
    }

    fn feedback_targets(&self) -> Vec<SocketAddr> {
        return match self.feedback_mode {
            FeedbackMode::Off => Vec::new(),
            FeedbackMode::Senders => self.recent_senders(),
            FeedbackMode::Configured => self.feedback_target.into_iter().collect(),
            FeedbackMode::Both => {
                let mut targets = self.recent_senders();
                if let Some(target) = self.feedback_target
                    && !targets.contains(&target)
                {
                    targets.push(target);
                }
                targets
            }
        };
    }

    fn recent_senders(&self) -> Vec<SocketAddr> {
        let now = Instant::now();
        return self
            .senders
            .iter()
            .filter(|(_, seen)| now.duration_since(**seen) <= FEEDBACK_TTL)
            .map(|(address, _)| *address)
            .collect();
    }

    fn remember_sender(&mut self, from: SocketAddr) {
        if !matches!(self.feedback_mode, FeedbackMode::Senders | FeedbackMode::Both) {
            return;
        }
        let now = Instant::now();
        if !self.senders.contains_key(&from) {
            self.pending_full.insert(from);
        }
        self.senders.insert(from, now);
        self.senders
            .retain(|_, seen| now.duration_since(*seen) <= FEEDBACK_TTL);
        while self.senders.len() > MAX_FEEDBACK_SENDERS {
            let oldest = self
                .senders
                .iter()
                .min_by_key(|(_, seen)| *seen)
                .map(|(address, _)| *address);
            let Some(address) = oldest else {
                break;
            };
            self.senders.remove(&address);
            self.pending_full.remove(&address);
        }
    }
}

fn resolve_feedback_target(host: &str, port: u16) -> Option<SocketAddr> {
    let host = host.trim();
    if host.is_empty() || port == 0 {
        return None;
    }
    return format!("{host}:{port}").to_socket_addrs().ok()?.next();
}

/// Whether a feedback target would feed the listener's own messages back into it.
fn is_self_target(target: SocketAddr, listener: SocketAddr) -> bool {
    if target == listener {
        return true;
    }
    if target.port() != listener.port() {
        return false;
    }
    return target.ip().is_loopback()
        || (target.ip().is_unspecified() && listener.ip().is_unspecified());
}

/// Addresses that changed, plus resets for addresses that disappeared.
fn diff(
    previous: &HashMap<String, FeedbackValue>,
    current: &[(String, FeedbackValue)],
) -> Vec<(String, FeedbackValue)> {
    let mut changes = Vec::new();
    let current_addresses: HashSet<&str> = current
        .iter()
        .map(|(address, _)| address.as_str())
        .collect();
    for (address, value) in current {
        if previous.get(address) != Some(value) {
            changes.push((address.clone(), value.clone()));
        }
    }
    for (address, value) in previous {
        if !current_addresses.contains(address.as_str()) {
            changes.push((address.clone(), value.reset()));
        }
    }
    changes.sort_by(|a, b| a.0.cmp(&b.0));
    return changes;
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::osc::OscControlConfig;

    #[test]
    fn invalid_prefix_is_reported_without_binding() {
        let mut registry = ControlRegistry::new();
        let config = ControlConfig {
            osc: OscControlConfig {
                enabled: true,
                bind: "127.0.0.1".to_string(),
                port: 0,
                prefix: "/a?b".to_string(),
                ..OscControlConfig::default()
            },
        };

        registry.apply(&config);

        assert!(matches!(registry.status(), ControlStatus::Error(_)));
    }

    #[test]
    fn diff_sends_changes_and_resets() {
        let none = HashMap::new();
        let current = vec![
            ("/a".to_string(), FeedbackValue::Int(1)),
            ("/b".to_string(), FeedbackValue::String("x".to_string())),
        ];
        assert_eq!(diff(&none, &current).len(), 2);

        let previous: HashMap<String, FeedbackValue> = current.iter().cloned().collect();
        assert!(diff(&previous, &current).is_empty());

        let changed = vec![
            ("/a".to_string(), FeedbackValue::Int(2)),
            ("/b".to_string(), FeedbackValue::String("x".to_string())),
        ];
        assert_eq!(
            diff(&previous, &changed),
            vec![("/a".to_string(), FeedbackValue::Int(2))]
        );

        let removed = vec![("/b".to_string(), FeedbackValue::String("x".to_string()))];
        assert_eq!(
            diff(&previous, &removed),
            vec![("/a".to_string(), FeedbackValue::Int(0))]
        );
    }

    #[test]
    fn senders_mode_tracks_a_sender_for_feedback() {
        let mut registry = ControlRegistry::new();
        registry.feedback_mode = FeedbackMode::Senders;
        let sender: SocketAddr = "127.0.0.1:9001".parse().unwrap();

        registry.remember_sender(sender);

        assert!(registry.feedback_targets().contains(&sender));
        assert!(registry.pending_full.contains(&sender));
    }

    #[test]
    fn configured_mode_uses_only_the_configured_target() {
        let mut registry = ControlRegistry::new();
        registry.feedback_mode = FeedbackMode::Configured;
        let target: SocketAddr = "127.0.0.1:9002".parse().unwrap();
        registry.feedback_target = Some(target);

        assert_eq!(registry.feedback_targets(), vec![target]);
    }

    #[test]
    fn off_mode_has_no_targets() {
        let mut registry = ControlRegistry::new();
        registry.feedback_mode = FeedbackMode::Off;
        registry.feedback_target = Some("127.0.0.1:9002".parse().unwrap());

        assert!(registry.feedback_targets().is_empty());
    }

    #[test]
    fn both_mode_uses_senders_and_the_configured_target() {
        let mut registry = ControlRegistry::new();
        registry.feedback_mode = FeedbackMode::Both;
        let target: SocketAddr = "127.0.0.1:9002".parse().unwrap();
        let sender: SocketAddr = "127.0.0.1:9003".parse().unwrap();
        registry.feedback_target = Some(target);
        registry.remember_sender(sender);

        let targets = registry.feedback_targets();
        assert!(targets.contains(&target));
        assert!(targets.contains(&sender));
    }

    #[test]
    fn both_mode_does_not_repeat_a_sender_that_is_the_target() {
        let mut registry = ControlRegistry::new();
        registry.feedback_mode = FeedbackMode::Both;
        let target: SocketAddr = "127.0.0.1:9002".parse().unwrap();
        registry.feedback_target = Some(target);
        registry.remember_sender(target);

        assert_eq!(
            registry
                .feedback_targets()
                .iter()
                .filter(|address| **address == target)
                .count(),
            1
        );
    }

    #[test]
    fn a_target_on_the_listener_port_is_self() {
        let listener: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        assert!(is_self_target(listener, listener));
        assert!(is_self_target(
            "127.0.0.2:9000".parse().unwrap(),
            listener
        ));
        assert!(!is_self_target(
            "192.168.1.20:9001".parse().unwrap(),
            listener
        ));
    }
}
