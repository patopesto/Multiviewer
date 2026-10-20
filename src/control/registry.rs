use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, Sender};

use super::command::Command;
use super::config::{ControlConfig, is_loopback_bind};
use super::osc::{OscServer, normalize_prefix};
use crate::engine::Engine;

/// Messages applied per frame; a UDP flood must not stall a render frame.
const MAX_COMMANDS_PER_FRAME: usize = 200;

#[derive(Clone, Debug, Default)]
pub enum ControlStatus {
    #[default]
    Stopped,
    Listening(SocketAddr),
    Error(String),
}

/// Runs on the UI thread with a borrow of the engine, so state replies are
/// encoded from the only copy of the state and nothing is cloned across threads.
pub type StateQuery = Box<dyn FnOnce(&Engine) + Send>;

/// What a control listener hands to the UI thread.
pub enum ControlMessage {
    Command(Command),
    Query(StateQuery),
}

/// The live control listeners. Owned by the engine on the UI thread; each
/// listener runs on its own thread and sends messages over a channel.
pub struct ControlRegistry {
    tx: Sender<ControlMessage>,
    rx: Receiver<ControlMessage>,
    osc: Option<OscServer>,
    status: ControlStatus,
    remote_confirmed: bool,
}

impl ControlRegistry {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            osc: None,
            status: ControlStatus::Stopped,
            remote_confirmed: false,
        }
    }

    /// Start, restart or stop the listeners to match `config`.
    pub fn apply(&mut self, config: &ControlConfig) {
        // Dropping the old server frees its port before the replacement binds.
        self.osc = None;
        self.status = ControlStatus::Stopped;

        let osc = &config.osc;
        if !osc.enabled {
            return;
        }
        if !is_loopback_bind(&osc.bind) && !self.remote_confirmed {
            self.status = ControlStatus::Error("Remote bind needs confirmation".to_string());
            return;
        }
        let prefix = match normalize_prefix(&osc.prefix) {
            Ok(prefix) => prefix,
            Err(e) => {
                self.status = ControlStatus::Error(e.to_string());
                return;
            }
        };
        match OscServer::start(&osc.bind, osc.port, prefix, self.tx.clone()) {
            Ok((server, addr)) => {
                self.osc = Some(server);
                self.status = ControlStatus::Listening(addr);
                tracing::info!("OSC control listening on {addr}");
            }
            Err(e) => {
                self.status = ControlStatus::Error(e.to_string());
                tracing::warn!("OSC control failed to start: {e}");
            }
        }
    }

    /// Allow a non-loopback bind for the rest of the session, then apply.
    pub fn confirm_remote(&mut self, config: &ControlConfig) {
        self.remote_confirmed = true;
        self.apply(config);
    }

    /// Take the messages received since the last call.
    pub fn drain(&self) -> Vec<ControlMessage> {
        let mut messages = Vec::new();
        while messages.len() < MAX_COMMANDS_PER_FRAME {
            let Ok(message) = self.rx.try_recv() else {
                return messages;
            };
            messages.push(message);
        }
        let mut dropped = 0;
        while self.rx.try_recv().is_ok() {
            dropped += 1;
        }
        if dropped > 0 {
            tracing::warn!("dropped {dropped} control messages (flood)");
        }
        return messages;
    }

    pub fn status(&self) -> &ControlStatus {
        return &self.status;
    }

    pub fn remote_confirmed(&self) -> bool {
        return self.remote_confirmed;
    }

    pub fn is_enabled(&self) -> bool {
        return self.osc.is_some();
    }

    #[cfg(test)]
    pub fn enqueue(&self, message: ControlMessage) {
        let _ = self.tx.send(message);
    }
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
            },
        };

        registry.apply(&config);

        assert!(matches!(registry.status(), ControlStatus::Error(_)));
    }

    #[test]
    fn remote_bind_needs_confirmation() {
        let mut registry = ControlRegistry::new();
        let config = ControlConfig {
            osc: OscControlConfig {
                enabled: true,
                bind: "0.0.0.0".to_string(),
                port: 0,
                ..OscControlConfig::default()
            },
        };

        registry.apply(&config);

        assert!(matches!(registry.status(), ControlStatus::Error(_)));
    }
}
