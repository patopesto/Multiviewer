use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig, SupportedStreamConfigRange};
use timecode_tools::ltc::{PcmChunk, Samples};
use timecode_tools::receiver::{LtcReceiver, ReceiverEvent, ReceiverOptions, TimecodeReceiver};
use timecode_tools::timecode::{FrameRate, Timecode as ReceivedTimecode};

use crate::sources::PollGate;

use super::source::ReceiverState;
use super::Timecode;

const POLL: Duration = Duration::from_millis(200);
const RETRY: Duration = Duration::from_secs(1);
const DISCOVERY: Duration = Duration::from_secs(2);
/// How long the decoder waits without a frame before it drops lock.
const LOST_AFTER: Duration = Duration::from_secs(3);

/// An audio input and the number of channels its supported configs offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LtcDevice {
    pub name: String,
    pub channels: u16,
}

/// Background audio input discovery, like the other protocol discoveries.
pub struct LtcDiscovery {
    devices: Arc<Mutex<Vec<LtcDevice>>>,
    gate: PollGate,
}

impl LtcDiscovery {
    pub fn start() -> Self {
        let devices = Arc::new(Mutex::new(Vec::new()));
        let cache = devices.clone();
        let gate = PollGate::with_interval(DISCOVERY);
        let wait = gate.clone();
        std::thread::Builder::new()
            .name("ltc-discovery".to_string())
            .spawn(move || {
                loop {
                    let next = scan_devices();
                    let mut current = cache.lock().unwrap();
                    if *current != next {
                        tracing::debug!(devices = next.len(), "LTC inputs changed");
                    }
                    *current = next;
                    drop(current);
                    wait.wait();
                }
            })
            .expect("spawn ltc-discovery");
        return Self { devices, gate };
    }

    pub fn list(&self) -> Vec<LtcDevice> {
        return self.devices.lock().unwrap().clone();
    }

    pub fn request_refresh(&self) {
        self.gate.request();
    }
}

/// Audio inputs the app can open. Called from the discovery thread, never the
/// engine tick.
fn scan_devices() -> Vec<LtcDevice> {
    let Ok(devices) = cpal::default_host().input_devices() else {
        return Vec::new();
    };
    let mut devices: Vec<LtcDevice> = devices
        .filter_map(|device| {
            let name = device.description().ok()?.name().to_owned();
            let channels = device
                .supported_input_configs()
                .ok()?
                .filter(|config| supported_format(config.sample_format()))
                .map(|config| config.channels())
                .max()?;
            return Some(LtcDevice { name, channels });
        })
        .collect();
    devices.sort_by(|a, b| a.name.cmp(&b.name));
    devices.dedup_by(|a, b| a.name == b.name);
    return devices;
}

fn supported_format(format: SampleFormat) -> bool {
    return matches!(
        format,
        SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32 | SampleFormat::I8 | SampleFormat::U8 | SampleFormat::U16 | SampleFormat::U32
    );
}

/// One-based channel must exist in the selected config.
fn channel_available(channel: u16, channels: u16) -> bool {
    return channel >= 1 && channel <= channels;
}

/// Pick a supported config that contains `channel` at the device's default
/// sample rate, falling back to the range's maximum rate.
fn select_config(device: &cpal::Device, channel: u16) -> Result<SupportedStreamConfig, String> {
    let default_rate = device.default_input_config().ok().map(|config| config.sample_rate());
    let configs = device.supported_input_configs().map_err(|error| format!("supported input configs unavailable: {error}"))?;
    return best_config(configs, default_rate, channel).ok_or_else(|| format!("channel {channel} is not available"));
}

fn best_config(configs: impl Iterator<Item = SupportedStreamConfigRange>, default_rate: Option<u32>, channel: u16) -> Option<SupportedStreamConfig> {
    let mut fallback = None;
    for config in configs {
        if !supported_format(config.sample_format()) || !channel_available(channel, config.channels()) {
            continue;
        }
        if let Some(rate) = default_rate
            && let Some(selected) = config.try_with_sample_rate(rate)
        {
            return Some(selected);
        }
        fallback.get_or_insert_with(|| config.with_max_sample_rate());
    }
    return fallback;
}

/// Receive LTC from an audio input until `running` clears, retrying a missing device.
pub(super) fn run(source: &str, device_name: &str, channel: u16, state: Arc<Mutex<ReceiverState>>, running: Arc<AtomicBool>) {
    if device_name.is_empty() {
        state.lock().unwrap().fail("no audio input selected".to_string());
        return;
    }
    while running.load(Ordering::Relaxed) {
        match open_stream(source, device_name, channel, &state) {
            Ok((stream, errors)) => {
                state.lock().unwrap().clear_error();
                let mut failed = false;
                while running.load(Ordering::Relaxed) {
                    if let Some(error) = errors.lock().unwrap().take() {
                        state.lock().unwrap().fail(error);
                        failed = true;
                        break;
                    }
                    std::thread::sleep(POLL);
                }
                drop(stream);
                if failed {
                    std::thread::sleep(RETRY);
                }
            }
            Err(error) => {
                state.lock().unwrap().fail(error);
                std::thread::sleep(RETRY);
            }
        }
    }
}

/// Last backend error reported by the audio callback.
type ErrorSlot = Arc<Mutex<Option<String>>>;

fn open_stream(
    source: &str,
    device_name: &str,
    channel: u16,
    state: &Arc<Mutex<ReceiverState>>,
) -> Result<(Stream, ErrorSlot), String> {
    let host = cpal::default_host();
    let devices = host.input_devices().map_err(|error| format!("audio inputs unavailable: {error}"))?;
    let Some(device) = devices.into_iter().find(|device| device.description().is_ok_and(|description| description.name() == device_name)) else {
        return Err(format!("audio input '{device_name}' not found"));
    };
    let supported = select_config(&device, channel).map_err(|error| format!("audio input '{device_name}': {error}"))?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let errors = Arc::new(Mutex::new(None));
    let stream = match sample_format {
        SampleFormat::F32 => build_input::<f32>(&device, config, channel, source, state, &errors),
        SampleFormat::I16 => build_input::<i16>(&device, config, channel, source, state, &errors),
        SampleFormat::I32 => build_input::<i32>(&device, config, channel, source, state, &errors),
        SampleFormat::I8 => build_input::<i8>(&device, config, channel, source, state, &errors),
        SampleFormat::U8 => build_input::<u8>(&device, config, channel, source, state, &errors),
        SampleFormat::U16 => build_input::<u16>(&device, config, channel, source, state, &errors),
        SampleFormat::U32 => build_input::<u32>(&device, config, channel, source, state, &errors),
        other => return Err(format!("unsupported audio sample format {other:?}")),
    }
    .map_err(|error| format!("audio input '{device_name}' failed: {error}"))?;
    stream.play().map_err(|error| format!("audio input '{device_name}' start failed: {error}"))?;
    return Ok((stream, errors));
}

/// Copy one interleaved channel into a mono buffer, converting to f32.
fn select_mono<T: Sample>(data: &[T], channels: u16, channel: u16, out: &mut Vec<f32>)
where
    f32: FromSample<T>,
{
    out.clear();
    let channels = usize::from(channels);
    let index = usize::from(channel).saturating_sub(1);
    out.reserve(data.len() / channels);
    for frame in data.chunks_exact(channels) {
        out.push(f32::from_sample(frame[index]));
    }
}

fn build_input<T>(
    device: &cpal::Device,
    config: StreamConfig,
    channel: u16,
    source: &str,
    state: &Arc<Mutex<ReceiverState>>,
    errors: &ErrorSlot,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let sample_rate = config.sample_rate;
    let channels = config.channels;
    let source = source.to_string();
    let state = state.clone();
    let error_slot = errors.clone();
    let mut receiver = LtcReceiver::new(ReceiverOptions {
        frame_rate: None,
        offset_frames: 0,
        lost_after: LOST_AFTER,
        channel: Some(1),
    });
    let mut mono = Vec::<f32>::new();
    let started = Instant::now();
    return device.build_input_stream(
        config,
        move |data: &[T], _info: &cpal::InputCallbackInfo| {
            select_mono(data, channels, channel, &mut mono);
            let chunk = PcmChunk {
                samples: Samples::F32(&mono),
                sample_rate,
                channels: 1,
                timestamp_ns: Some(started.elapsed().as_nanos() as u64),
            };
            let events = match receiver.push(chunk) {
                Ok(events) => events,
                Err(error) => {
                    tracing::warn!(source = %source, %error, "ltc decode failed");
                    return;
                }
            };
            for event in events {
                let received = match event {
                    ReceiverEvent::Lock(timecode) | ReceiverEvent::Jump(timecode) => timecode,
                    ReceiverEvent::Frame { timecode, .. } => timecode,
                    ReceiverEvent::Lost => continue,
                };
                let Some(timecode) = convert(received) else {
                    continue;
                };
                tracing::debug!(source = %source, timecode = %timecode.label(), "ltc timecode");
                state.lock().unwrap().publish(timecode);
            }
        },
        move |error: cpal::Error| {
            if matches!(error.kind(), cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied) {
                tracing::warn!(%error, "ltc stream warning");
            } else {
                *error_slot.lock().unwrap() = Some(error.to_string());
            }
        },
        None,
    );
}

fn convert(received: ReceivedTimecode) -> Option<Timecode> {
    let code = match received.frame_rate? {
        FrameRate::Fps24 => 0,
        FrameRate::Fps25 => 1,
        FrameRate::Fps2997Drop => 2,
        FrameRate::Fps30 => 3,
    };
    return Timecode::from_rate(received.label.hours, received.label.minutes, received.label.seconds, received.label.frames, code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use timecode_tools::synth::{SynthConfig, generate};
    use timecode_tools::timecode::Timecode as ReceivedTimecode;

    fn decode(pcm: &[i16], sample_rate: u32, channels: u16, channel: u16) -> (Vec<ReceiverEvent>, Option<u32>) {
        let mut receiver = LtcReceiver::new(ReceiverOptions {
            frame_rate: None,
            offset_frames: 0,
            lost_after: LOST_AFTER,
            channel: Some(1),
        });
        let mut events = Vec::new();
        let mut mono = Vec::new();
        for chunk in pcm.chunks(1024) {
            select_mono(chunk, channels, channel, &mut mono);
            let samples = PcmChunk {
                samples: Samples::F32(&mono),
                sample_rate,
                channels: 1,
                timestamp_ns: None,
            };
            events.extend(receiver.push(samples).unwrap());
        }
        return (events, receiver.channel());
    }

    fn last_frame(events: &[ReceiverEvent]) -> Option<ReceivedTimecode> {
        let mut last = None;
        for event in events {
            if let ReceiverEvent::Frame { timecode, .. } = event {
                last = Some(*timecode);
            }
        }
        return last;
    }

    #[test]
    fn decodes_each_rate() {
        for (rate, nominal_fps, drop_frame, fps) in [
            (24.0, 24, false, 24.0),
            (25.0, 25, false, 25.0),
            (30_000.0 / 1001.0, 30, true, 30_000.0 / 1001.0),
            (30.0, 30, false, 30.0),
        ] {
            let signal = generate(SynthConfig {
                sample_rate: 48_000,
                rate,
                nominal_fps,
                drop_frame,
                frame_count: 10,
                ..Default::default()
            });
            let (events, _) = decode(&signal.pcm, 48_000, 1, 1);
            let received = last_frame(&events).expect("a locked frame");
            let timecode = convert(received).expect("a known rate");
            assert_eq!(timecode.fps, fps, "rate {rate}");
            assert_eq!(timecode.drop_frame, drop_frame, "rate {rate}");
            assert_eq!(timecode.frames, 9, "the last synthetic frame");
        }
    }

    #[test]
    fn reads_only_the_selected_channel() {
        let signal = generate(SynthConfig {
            sample_rate: 48_000,
            rate: 25.0,
            nominal_fps: 25,
            frame_count: 10,
            ..Default::default()
        });
        let mut stereo = Vec::with_capacity(signal.pcm.len() * 2);
        for sample in &signal.pcm {
            stereo.push(0);
            stereo.push(*sample);
        }
        let (events, _) = decode(&stereo, 48_000, 2, 2);
        let received = last_frame(&events).expect("a locked frame");
        assert_eq!(convert(received).unwrap().frames, 9);

        let (silent, _) = decode(&stereo, 48_000, 2, 1);
        assert!(last_frame(&silent).is_none(), "channel 1 carries silence");
    }

    #[test]
    fn reads_a_channel_above_32() {
        let signal = generate(SynthConfig {
            sample_rate: 48_000,
            rate: 25.0,
            nominal_fps: 25,
            frame_count: 10,
            ..Default::default()
        });
        let mut many = Vec::with_capacity(signal.pcm.len() * 64);
        for sample in &signal.pcm {
            for channel in 0..64 {
                many.push(if channel == 39 { *sample } else { 0 });
            }
        }
        let (events, _) = decode(&many, 48_000, 64, 40);
        let received = last_frame(&events).expect("a locked frame");
        assert_eq!(convert(received).unwrap().frames, 9);

        let (silent, _) = decode(&many, 48_000, 64, 1);
        assert!(last_frame(&silent).is_none(), "channel 1 carries silence");
    }

    #[test]
    fn channel_selection_is_validated() {
        assert!(channel_available(1, 1));
        assert!(channel_available(2, 2));
        assert!(channel_available(40, 64));
        assert!(!channel_available(2, 1));
        assert!(!channel_available(0, 2));
        assert!(!channel_available(65, 64));
    }

    #[test]
    fn selects_a_config_that_contains_the_channel() {
        let stereo = SupportedStreamConfigRange::new(2, 44_100, 48_000, cpal::SupportedBufferSize::Unknown, SampleFormat::F32);
        let eight = SupportedStreamConfigRange::new(8, 44_100, 48_000, cpal::SupportedBufferSize::Unknown, SampleFormat::F32);
        let many = SupportedStreamConfigRange::new(64, 44_100, 48_000, cpal::SupportedBufferSize::Unknown, SampleFormat::F32);

        let selected = best_config([stereo, eight].into_iter(), Some(48_000), 3).unwrap();
        assert_eq!((selected.channels(), selected.sample_rate()), (8, 48_000));

        let selected = best_config([stereo, eight].into_iter(), Some(48_000), 1).unwrap();
        assert_eq!(selected.channels(), 2);

        let selected = best_config([many].into_iter(), Some(48_000), 40).unwrap();
        assert_eq!(selected.channels(), 64);

        let selected = best_config([eight].into_iter(), Some(96_000), 3).unwrap();
        assert_eq!(selected.sample_rate(), 48_000);

        let unsupported = SupportedStreamConfigRange::new(8, 44_100, 48_000, cpal::SupportedBufferSize::Unknown, SampleFormat::I24);
        assert!(best_config([unsupported].into_iter(), Some(48_000), 3).is_none());
        assert!(best_config([stereo, eight].into_iter(), Some(48_000), 9).is_none());
    }

    #[test]
    fn silence_never_locks() {
        let (events, _) = decode(&vec![0i16; 48_000], 48_000, 1, 1);
        assert!(last_frame(&events).is_none());
    }

    #[test]
    fn drop_frame_labels_convert() {
        let received = ReceivedTimecode {
            label: timecode_tools::label::Label {
                hours: 0,
                minutes: 1,
                seconds: 0,
                frames: 2,
                drop_frame: true,
            },
            frame_rate: Some(FrameRate::Fps2997Drop),
            locked: true,
            timestamp_ns: None,
            sample_index: None,
            channel: Some(1),
        };
        let timecode = convert(received).unwrap();
        assert_eq!(timecode.label(), "00:01:00;02");
        assert!((timecode.fps - 29.97).abs() < 1e-2);
    }
}
