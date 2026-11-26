use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::input::{SharedInput, TimecodeHub, TimecodeProtocol};
use super::ltc;
use super::{TimecodeDisplay, TimecodeStatus};
use super::super::{CpuFrame, Frame, PixelFormat, SourceConfig, SourceStats, TextStyle, VideoSource};

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

/// Inputs the Timecode settings panel offers for this machine.
#[derive(Clone, Debug, Default)]
pub struct TimecodeInputs {
    pub mtc_ports: Vec<String>,
    pub ltc_devices: Vec<ltc::LtcDevice>,
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

struct Cached {
    value: Option<String>,
    generation: u64,
    frame: Frame,
    seq: u64,
}

/// How the card is presented. Changing it re-renders on the next frame; it
/// never touches the receiver.
struct Presentation {
    display: TimecodeDisplay,
    style: TextStyle,
    generation: u64,
}

/// A timecode display. The shared input publishes the last value, and the
/// frame is re-rendered only when the formatted value or the presentation
/// changes.
pub struct TimecodeSource {
    presentation: Mutex<Presentation>,
    hub: Arc<TimecodeHub>,
    input: Arc<SharedInput>,
    cache: Mutex<Cached>,
    stats: Arc<Mutex<SourceStats>>,
}

impl TimecodeSource {
    pub fn spawn(hub: Arc<TimecodeHub>, config: &TimecodeSourceConfig) -> Self {
        let style = capped_style(config);
        let input = hub.attach(&config.input);

        let frame = text_frame(&style, config.display.placeholder(), 0);
        let mut stats = SourceStats::new();
        stats.record_frame(style.width, style.height, PixelFormat::Rgba8.label(), 0.0);
        return Self {
            presentation: Mutex::new(Presentation {
                display: config.display,
                style,
                generation: 0,
            }),
            hub,
            input,
            cache: Mutex::new(Cached {
                value: None,
                generation: 0,
                frame,
                seq: 0,
            }),
            stats: Arc::new(Mutex::new(stats)),
        };
    }

    /// The picture for the current value and presentation, re-rendered only
    /// when either changed.
    fn current_frame(&self) -> Frame {
        let status = self.input.status();
        let presentation = self.presentation.lock().unwrap();
        let value = status.timecode.map(|timecode| presentation.display.format(&timecode));
        let mut cache = self.cache.lock().unwrap();
        if cache.value != value || cache.generation != presentation.generation {
            let text = value.as_deref().unwrap_or_else(|| presentation.display.placeholder());
            let frame = text_frame(&presentation.style, text, cache.seq + 1);
            cache.seq += 1;
            cache.frame = frame;
            cache.value = value;
            cache.generation = presentation.generation;
            let mut stats = self.stats.lock().unwrap();
            stats.record_frame(
                presentation.style.width,
                presentation.style.height,
                PixelFormat::Rgba8.label(),
                status.timecode.map_or(0.0, |tc| tc.fps),
            );
        }
        return cache.frame.clone();
    }
}

fn capped_style(config: &TimecodeSourceConfig) -> TextStyle {
    let (max_width, max_height) = TimecodeSourceConfig::MAX_SIZE;
    let mut style = config.style.clone().bounded();
    style.width = style.width.min(max_width);
    style.height = style.height.min(max_height);
    return style;
}

impl Drop for TimecodeSource {
    fn drop(&mut self) {
        self.hub.detach(&self.input);
    }
}

impl VideoSource for TimecodeSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        return Some(self.current_frame());
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }

    fn alive(&self) -> bool {
        return self.input.alive();
    }

    fn apply_config(&self, config: &SourceConfig) -> bool {
        let SourceConfig::Timecode(config) = config else {
            return false;
        };
        // A different input rebinds the receiver, which only a reopen can do.
        if config.input != *self.input.key() {
            return false;
        }
        let style = capped_style(config);
        let mut presentation = self.presentation.lock().unwrap();
        if presentation.display == config.display && presentation.style == style {
            return true;
        }
        presentation.display = config.display;
        presentation.style = style;
        presentation.generation += 1;
        return true;
    }

    fn timecode(&self) -> Option<TimecodeStatus> {
        return Some(self.input.status());
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
    use crate::sources::TimecodeState;

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
        let source = TimecodeSource::spawn(Arc::new(TimecodeHub::default()), &config);
        assert_eq!(
            source.timecode(),
            Some(TimecodeStatus {
                timecode: None,
                state: TimecodeState::Waiting,
            })
        );

        source.input.state().lock().unwrap().publish(test_timecode());
        assert_eq!(
            source.timecode(),
            Some(TimecodeStatus {
                timecode: Some(test_timecode()),
                state: TimecodeState::Receiving,
            })
        );
    }

    #[test]
    fn two_cards_on_one_input_share_the_received_value() {
        let hub = Arc::new(TimecodeHub::default());
        let config = TimecodeSourceConfig {
            input: TimecodeProtocol::ArtNet {
                bind: "127.0.0.1".to_string(),
                port: 0,
                stream_id: 0,
            },
            ..Default::default()
        };
        let first = TimecodeSource::spawn(hub.clone(), &config);
        let second = TimecodeSource::spawn(hub.clone(), &config);
        assert!(Arc::ptr_eq(&first.input, &second.input));

        first.input.state().lock().unwrap().publish(test_timecode());
        assert_eq!(second.timecode().unwrap().timecode, Some(test_timecode()));
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
        let input: TimecodeProtocol = serde_json::from_str(r#"{"protocol":"Mtc","port":"IAC Driver Bus 1","id":"coremidi:1234"}"#).unwrap();
        assert_eq!(
            input,
            TimecodeProtocol::Mtc {
                port: "IAC Driver Bus 1".to_string(),
            }
        );
    }

    #[test]
    fn the_ltc_input_round_trips_through_a_project() {
        let config = TimecodeSourceConfig {
            input: TimecodeProtocol::Ltc {
                device: "Scarlett 2i2 USB".to_string(),
                channel: 2,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"protocol\":\"Ltc\""));
        assert_eq!(serde_json::from_str::<TimecodeSourceConfig>(&json).unwrap(), config);
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
        let source = TimecodeSource::spawn(Arc::new(TimecodeHub::default()), &config);
        let cache = source.cache.lock().unwrap();
        let Frame::Cpu(frame) = &cache.frame else {
            unreachable!("timecode sources publish CPU frames");
        };
        assert_eq!((frame.w, frame.h), TimecodeSourceConfig::MAX_SIZE);
    }

    #[test]
    fn a_presentation_change_rerenders_without_touching_the_input() {
        let mut config = TimecodeSourceConfig {
            input: TimecodeProtocol::ArtNet {
                bind: "127.0.0.1".to_string(),
                port: 0,
                stream_id: 0,
            },
            ..Default::default()
        };
        let source = TimecodeSource::spawn(Arc::new(TimecodeHub::default()), &config);
        let input = source.input.clone();
        let seq = source.current_frame().seq();
        assert_eq!(seq, 0);

        config.display = TimecodeDisplay::Seconds;
        config.style.text_color = [255, 0, 0, 255];
        assert!(source.apply_config(&SourceConfig::Timecode(config.clone())));
        assert!(Arc::ptr_eq(&source.input, &input), "a style edit must not rebind the receiver");
        assert!(source.current_frame().seq() > seq, "the new presentation re-renders");
    }

    #[test]
    fn an_identical_presentation_does_not_rerender() {
        let config = TimecodeSourceConfig {
            input: TimecodeProtocol::ArtNet {
                bind: "127.0.0.1".to_string(),
                port: 0,
                stream_id: 0,
            },
            ..Default::default()
        };
        let source = TimecodeSource::spawn(Arc::new(TimecodeHub::default()), &config);
        assert!(source.apply_config(&SourceConfig::Timecode(config)));
        assert_eq!(source.current_frame().seq(), 0);
    }

    #[test]
    fn an_input_change_is_not_adopted() {
        let mut config = TimecodeSourceConfig {
            input: TimecodeProtocol::ArtNet {
                bind: "127.0.0.1".to_string(),
                port: 0,
                stream_id: 0,
            },
            ..Default::default()
        };
        let source = TimecodeSource::spawn(Arc::new(TimecodeHub::default()), &config);
        config.input = TimecodeProtocol::Mtc {
            port: "IAC Driver Bus 1".to_string(),
        };
        assert!(!source.apply_config(&SourceConfig::Timecode(config)), "a new input reopens the source");
    }
}
