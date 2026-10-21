use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rosc::{OscMessage, OscPacket, OscType, decoder};

use super::super::command::{Command, Selector};
use crate::config::TextureMode;

const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// A UDP OSC listener. Bind happens on the caller's thread so a bad address or
/// a taken port is reported immediately; the receive loop runs on its own.
pub struct OscServer {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl OscServer {
    pub fn start(
        bind: &str,
        port: u16,
        prefix: String,
        tx: Sender<Command>,
    ) -> Result<(Self, SocketAddr), std::io::Error> {
        let socket = UdpSocket::bind((bind, port))?;
        socket.set_read_timeout(Some(POLL_INTERVAL))?;
        let addr = socket.local_addr()?;
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = running.clone();
        let thread = thread::Builder::new()
            .name("osc-control".to_string())
            .spawn(move || run_server(socket, tx, prefix, thread_running))?;
        return Ok((Self { running, thread: Some(thread) }, addr));
    }
}

impl Drop for OscServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_server(socket: UdpSocket, tx: Sender<Command>, prefix: String, running: Arc<AtomicBool>) {
    let mut buffer = [0u8; 65535];
    while running.load(Ordering::Acquire) {
        let (len, from) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(e) => {
                tracing::error!("OSC listener stopped: {e}");
                break;
            }
        };
        let packet = match decoder::decode_udp(&buffer[..len]) {
            Ok((_, packet)) => packet,
            Err(e) => {
                tracing::debug!(from = %from, "ignoring malformed OSC packet: {e}");
                continue;
            }
        };
        handle_packet(packet, from, &tx, &prefix);
    }
}

fn handle_packet(packet: OscPacket, from: SocketAddr, tx: &Sender<Command>, prefix: &str) {
    match packet {
        OscPacket::Message(message) => handle_message(message, from, tx, prefix),
        OscPacket::Bundle(bundle) => {
            for packet in bundle.content {
                handle_packet(packet, from, tx, prefix);
            }
        }
    }
}

fn handle_message(message: OscMessage, from: SocketAddr, tx: &Sender<Command>, prefix: &str) {
    let Some(address) = strip_prefix(&message.addr, prefix) else {
        tracing::debug!(
            from = %from,
            addr = %message.addr,
            "ignoring OSC address outside the configured prefix"
        );
        return;
    };
    match parse_message(address, &message.args) {
        Some(command) => {
            tracing::debug!(from = %from, command = ?command, "OSC command");
            let _ = tx.send(command);
        }
        None => tracing::debug!(from = %from, addr = %message.addr, "ignoring unknown OSC address"),
    }
}

/// Strip the configured prefix. The remainder must start with `/`, so a
/// prefix never matches a sibling address that merely shares its text.
fn strip_prefix<'a>(addr: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return Some(addr);
    }
    return match addr.strip_prefix(prefix) {
        Some(rest) if rest.starts_with('/') => Some(rest),
        _ => None,
    };
}

fn parse_message(addr: &str, args: &[OscType]) -> Option<Command> {
    let parts: Vec<&str> = addr.strip_prefix('/')?.split('/').collect();
    return match *parts.first()? {
        "source" => parse_source(&parts[1..], args),
        "nudge" => Some(Command::Nudge {
            dx: arg_f32(args.first()?)?,
            dy: arg_f32(args.get(1)?)?,
        }),
        "canvas" if parts.get(1) == Some(&"size") => Some(Command::SetCanvasSize {
            width: arg_u32(args.first()?)?,
            height: arg_u32(args.get(1)?)?,
        }),
        "view" => match *parts.get(1)? {
            "zoom" => Some(Command::Zoom {
                factor: arg_f32(args.first()?)?,
            }),
            "recenter" => Some(Command::RecenterView),
            _ => None,
        },
        "ui" if parts.get(1) == Some(&"toggle") => Some(Command::ToggleUi),
        "output" => parse_output(&parts[1..], args),
        "project" if parts.get(1) == Some(&"save") => Some(Command::SaveProject),
        _ => None,
    };
}

fn parse_source(rest: &[&str], args: &[OscType]) -> Option<Command> {
    match rest {
        ["add"] => return Some(Command::AddSource),
        ["next"] => return Some(Command::SelectNext),
        ["prev"] | ["previous"] => return Some(Command::SelectPrevious),
        ["exit_expanded"] => return Some(Command::ExitExpanded),
        ["select"] => return Some(Command::SelectSource(selector_arg(args.first()?)?)),
        ["deselect"] => return Some(Command::ClearSelection),
        _ => {}
    }

    let source = parse_selector(rest.first()?)?;
    return match *rest.get(1)? {
        "x" => Some(Command::SetSourceX {
            source,
            value: arg_f32(args.first()?)?,
        }),
        "y" => Some(Command::SetSourceY {
            source,
            value: arg_f32(args.first()?)?,
        }),
        "w" | "width" => Some(Command::SetSourceWidth {
            source,
            value: arg_u32(args.first()?)?,
        }),
        "h" | "height" => Some(Command::SetSourceHeight {
            source,
            value: arg_u32(args.first()?)?,
        }),
        "size" => Some(Command::SetSourceSize {
            source,
            width: arg_u32(args.first()?)?,
            height: arg_u32(args.get(1)?)?,
        }),
        "z" => Some(Command::SetSourceZ {
            source,
            value: arg_i32(args.first()?)?,
        }),
        "mode" => Some(Command::SetSourceMode {
            source,
            mode: arg_mode(args.first()?)?,
        }),
        "flip_h" => Some(Command::SetSourceFlipH {
            source,
            value: arg_bool(args.first()?)?,
        }),
        "flip_v" => Some(Command::SetSourceFlipV {
            source,
            value: arg_bool(args.first()?)?,
        }),
        "select" => Some(Command::SelectSource(source)),
        "expand" => Some(Command::ExpandSource(source)),
        "remove" => Some(Command::RemoveSource(source)),
        _ => None,
    };
}

fn parse_output(rest: &[&str], args: &[OscType]) -> Option<Command> {
    let output = parse_selector(rest.first()?)?;
    return match *rest.get(1)? {
        "enabled" => Some(Command::SetOutputEnabled {
            output,
            enabled: arg_bool(args.first()?)?,
        }),
        "restart" => Some(Command::RestartOutput(output)),
        _ => None,
    };
}

fn parse_selector(text: &str) -> Option<Selector> {
    if text.is_empty() {
        return None;
    }
    return match text.parse::<usize>() {
        Ok(index) => Some(Selector::Index(index)),
        Err(_) => Some(Selector::Key(text.to_string())),
    };
}

fn selector_arg(arg: &OscType) -> Option<Selector> {
    if let OscType::String(text) = arg {
        return parse_selector(text);
    }
    let index = arg_i32(arg)?;
    return (index >= 0).then(|| Selector::Index(index as usize));
}

fn arg_f32(arg: &OscType) -> Option<f32> {
    return match arg {
        OscType::Float(value) => Some(*value),
        OscType::Double(value) => Some(*value as f32),
        OscType::Int(value) => Some(*value as f32),
        _ => None,
    };
}

fn arg_i32(arg: &OscType) -> Option<i32> {
    return match arg {
        OscType::Int(value) => Some(*value),
        OscType::Long(value) => Some(*value as i32),
        OscType::Float(value) => Some(*value as i32),
        OscType::Double(value) => Some(*value as i32),
        _ => None,
    };
}

fn arg_u32(arg: &OscType) -> Option<u32> {
    return arg_i32(arg).filter(|value| *value >= 0).map(|value| value as u32);
}

fn arg_bool(arg: &OscType) -> Option<bool> {
    return match arg {
        OscType::Bool(value) => Some(*value),
        OscType::Int(value) => Some(*value != 0),
        OscType::Float(value) => Some(*value != 0.0),
        _ => None,
    };
}

fn arg_mode(arg: &OscType) -> Option<TextureMode> {
    if let OscType::String(name) = arg {
        return match name.to_ascii_lowercase().as_str() {
            "fit" => Some(TextureMode::Fit),
            "fill" => Some(TextureMode::Fill),
            "stretch" => Some(TextureMode::Stretch),
            _ => None,
        };
    }
    return match arg_i32(arg)? {
        0 => Some(TextureMode::Fit),
        1 => Some(TextureMode::Fill),
        2 => Some(TextureMode::Stretch),
        _ => None,
    };
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use rosc::encoder;

    use super::*;

    #[test]
    fn parses_source_position() {
        assert_eq!(
            parse_message("/source/2/x", &[OscType::Float(100.0)]),
            Some(Command::SetSourceX {
                source: Selector::Index(2),
                value: 100.0,
            })
        );
    }

    #[test]
    fn parses_named_selector_and_mode() {
        assert_eq!(
            parse_message(
                "/source/CamA/mode",
                &[OscType::String("Fill".to_string())],
            ),
            Some(Command::SetSourceMode {
                source: Selector::Key("CamA".to_string()),
                mode: TextureMode::Fill,
            })
        );
    }

    #[test]
    fn parses_source_actions() {
        assert_eq!(parse_message("/source/add", &[]), Some(Command::AddSource));
        assert_eq!(
            parse_message("/source/next", &[]),
            Some(Command::SelectNext)
        );
        assert_eq!(
            parse_message("/source/prev", &[]),
            Some(Command::SelectPrevious)
        );
        assert_eq!(
            parse_message("/source/previous", &[]),
            Some(Command::SelectPrevious)
        );
        assert_eq!(
            parse_message("/source/1/width", &[OscType::Int(640)]),
            Some(Command::SetSourceWidth {
                source: Selector::Index(1),
                value: 640,
            })
        );
        assert_eq!(
            parse_message("/source/1/height", &[OscType::Int(360)]),
            Some(Command::SetSourceHeight {
                source: Selector::Index(1),
                value: 360,
            })
        );
        assert_eq!(
            parse_message("/source/select", &[OscType::String("CamA".to_string())]),
            Some(Command::SelectSource(Selector::Key("CamA".to_string())))
        );
        assert_eq!(
            parse_message("/source/1/remove", &[]),
            Some(Command::RemoveSource(Selector::Index(1)))
        );
        assert_eq!(
            parse_message("/source/deselect", &[]),
            Some(Command::ClearSelection)
        );
    }

    #[test]
    fn parses_output_and_view() {
        assert_eq!(
            parse_message("/output/1/enabled", &[OscType::Int(0)]),
            Some(Command::SetOutputEnabled {
                output: Selector::Index(1),
                enabled: false,
            })
        );
        assert_eq!(
            parse_message("/view/recenter", &[]),
            Some(Command::RecenterView)
        );
    }

    #[test]
    fn rejects_unknown_addresses_and_bad_args() {
        assert_eq!(parse_message("/nope", &[]), None);
        assert_eq!(parse_message("/source/1/x", &[OscType::String("x".into())]), None);
        assert_eq!(parse_message("source/1/x", &[OscType::Float(1.0)]), None);
    }

    #[test]
    fn strips_a_configured_prefix() {
        assert_eq!(
            strip_prefix("/multiviewer/source/1/x", "/multiviewer"),
            Some("/source/1/x")
        );
        assert_eq!(
            strip_prefix("/multiviewer/source/1/x", ""),
            Some("/multiviewer/source/1/x")
        );
        assert_eq!(strip_prefix("/mvx/source/1/x", "/mv"), None);
        assert_eq!(strip_prefix("/other/source/1/x", "/mv"), None);
    }

    /// The whole loop: bind, receive a real datagram, decode, hand off a command.
    #[test]
    fn listener_decodes_a_datagram_into_a_command() {
        let (tx, rx) = mpsc::channel();
        let (_server, addr) =
            OscServer::start("127.0.0.1", 0, "/multiviewer".to_string(), tx).unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let packet = OscPacket::Message(OscMessage {
            addr: "/multiviewer/source/1/x".to_string(),
            args: vec![OscType::Float(42.0)],
        });
        sender
            .send_to(&encoder::encode(&packet).unwrap(), addr)
            .unwrap();

        let command = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            command,
            Command::SetSourceX {
                source: Selector::Index(1),
                value: 42.0,
            }
        );
    }
}
