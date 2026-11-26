use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::input::ReceiverState;
use super::Timecode;

const RECEIVE_TIMEOUT: Duration = Duration::from_millis(200);

// Static values in the packet
const ARTNET_ID: &[u8; 8] = b"Art-Net\0";
const ART_TIMECODE: u16 = 0x9700; // opcode
const PACKET_LEN: usize = TYPE + 1;

// Byte offsets in packet
const OP_CODE: usize = 8;
const STREAM_ID: usize = 13;
const FRAMES: usize = 14;
const SECONDS: usize = 15;
const MINUTES: usize = 16;
const HOURS: usize = 17;
const TYPE: usize = 18;

/// Receive Art-Net timecode until `running` clears.
pub(super) fn run(source: &str, bind: &str, port: u16, stream_id: u8, state: Arc<Mutex<ReceiverState>>, running: Arc<AtomicBool>) {
    let socket = match UdpSocket::bind((bind, port)) {
        Ok(socket) => socket,
        Err(error) => {
            state.lock().unwrap().fail(format!("bind {bind}:{port} failed: {error}"));
            return;
        }
    };
    if let Err(error) = socket.set_read_timeout(Some(RECEIVE_TIMEOUT)) {
        state.lock().unwrap().fail(format!("socket timeout setup failed: {error}"));
        return;
    }

    let mut packet = [0u8; 256];
    while running.load(Ordering::Relaxed) {
        match socket.recv_from(&mut packet) {
            Ok((len, _)) => {
                if let Some(timecode) = parse(&packet[..len], stream_id) {
                    tracing::debug!(source, timecode = %timecode.label(), "artnet timecode");
                    state.lock().unwrap().publish(timecode);
                }
            }
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(error) => {
                tracing::warn!(source, %error, "artnet receive failed");
                std::thread::sleep(RECEIVE_TIMEOUT);
            }
        }
    }
}

/// Parse an `ArtTimeCode` packet.
pub(super) fn parse(packet: &[u8], stream_id: u8) -> Option<Timecode> {
    if packet.len() < PACKET_LEN || !packet.starts_with(ARTNET_ID) {
        return None;
    }
    if u16::from_le_bytes([packet[OP_CODE], packet[OP_CODE + 1]]) != ART_TIMECODE {
        return None;
    }
    // StreamId replaced Filler2 in Art-Net 4 revision DK; older senders leave
    // the byte at 0, which reads as the master stream.
    if packet[STREAM_ID] != stream_id {
        return None;
    }

    let timecode = Timecode::from_rate(packet[HOURS], packet[MINUTES], packet[SECONDS], packet[FRAMES], packet[TYPE])?;
    let dropped = timecode.nominal_fps() * 2 / 30;
    if timecode.drop_frame && timecode.seconds == 0 && !timecode.minutes.is_multiple_of(10) && u64::from(timecode.frames) < dropped {
        return None;
    }
    return Some(timecode);
}

#[cfg(test)]
pub(super) fn packet(stream_id: u8, frames: u8, seconds: u8, minutes: u8, hours: u8, kind: u8) -> Vec<u8> {
    // ID, OpCode, ProtVer 0/14, Filler1, StreamId, Frames, Seconds, Minutes,
    // Hours, Type.
    let mut packet = Vec::with_capacity(19);
    packet.extend_from_slice(b"Art-Net\0");
    packet.extend_from_slice(&ART_TIMECODE.to_le_bytes());
    packet.extend_from_slice(&[0, 14, 0, stream_id, frames, seconds, minutes, hours, kind]);
    return packet;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_timecode_type_and_rate() {
        let film = parse(&packet(0, 23, 1, 2, 3, 0), 0).unwrap();
        assert_eq!(film.label(), "03:02:01:23");
        assert_eq!(film.fps, 24.0);
        assert!(!film.drop_frame);

        let ebu = parse(&packet(0, 24, 1, 2, 3, 1), 0).unwrap();
        assert_eq!(ebu.label(), "03:02:01:24");
        assert_eq!(ebu.fps, 25.0);

        let drop_frame = parse(&packet(0, 29, 1, 2, 3, 2), 0).unwrap();
        assert!(drop_frame.drop_frame);
        assert!((drop_frame.fps - 29.970_03).abs() < 1e-4);
        assert_eq!(drop_frame.label(), "03:02:01;29");

        let smpte = parse(&packet(0, 29, 1, 2, 3, 3), 0).unwrap();
        assert!(!smpte.drop_frame);
        assert_eq!(smpte.fps, 30.0);
        assert_eq!(smpte.label(), "03:02:01:29");
    }

    #[test]
    fn ignores_the_protocol_version_and_filler_bytes() {
        let mut packet = packet(0, 1, 2, 3, 4, 1);
        packet[10] = 0;
        packet[11] = 15;
        packet[12] = 0xAA;
        assert!(parse(&packet, 0).is_some());
    }

    #[test]
    fn rejects_packets_that_are_not_art_timecode() {
        assert!(parse(b"short", 0).is_none());

        let mut wrong_id = packet(0, 0, 0, 0, 0, 1);
        wrong_id[0] = b'X';
        assert!(parse(&wrong_id, 0).is_none());

        let mut wrong_opcode = packet(0, 0, 0, 0, 0, 1);
        wrong_opcode[8] = 0x00;
        wrong_opcode[9] = 0x98;
        assert!(parse(&wrong_opcode, 0).is_none());

        assert!(parse(&packet(1, 0, 0, 0, 0, 1), 0).is_none(), "another stream_id");
        assert!(parse(&packet(0, 0, 0, 0, 0, 4), 0).is_none(), "unknown type");
    }

    #[test]
    fn rejects_out_of_range_fields() {
        assert!(parse(&packet(0, 24, 0, 0, 0, 0), 0).is_none(), "film has no frame 24");
        assert!(parse(&packet(0, 25, 0, 0, 0, 1), 0).is_none(), "EBU has no frame 25");
        assert!(parse(&packet(0, 30, 0, 0, 0, 2), 0).is_none(), "drop frame has no frame 30");
        assert!(parse(&packet(0, 0, 60, 0, 0, 1), 0).is_none(), "seconds");
        assert!(parse(&packet(0, 0, 0, 60, 0, 1), 0).is_none(), "minutes");
        assert!(parse(&packet(0, 0, 0, 0, 24, 1), 0).is_none(), "hours");
    }

    #[test]
    fn rejects_drop_frame_numbers_that_smpte_skips() {
        assert!(parse(&packet(0, 0, 0, 1, 0, 2), 0).is_none(), "frame 0 of minute 1 is skipped");
        assert!(parse(&packet(0, 1, 0, 1, 0, 2), 0).is_none(), "frame 1 of minute 1 is skipped");
        assert!(parse(&packet(0, 2, 0, 1, 0, 2), 0).is_some(), "frame 2 exists");
        assert!(parse(&packet(0, 0, 0, 10, 0, 2), 0).is_some(), "the tenth minute keeps frame 0");
        assert!(parse(&packet(0, 0, 1, 1, 0, 2), 0).is_some(), "frame 0 after a second is fine");
        assert!(parse(&packet(0, 0, 0, 1, 0, 3), 0).is_some(), "non-drop keeps frame 0");
    }

    #[test]
    fn accepts_trailing_bytes() {
        let mut packet = packet(0, 1, 2, 3, 4, 1);
        packet.extend_from_slice(&[0; 8]);
        assert!(parse(&packet, 0).is_some());
    }
}
