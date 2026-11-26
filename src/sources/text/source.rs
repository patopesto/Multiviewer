use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::super::{CpuFrame, Frame, PixelFormat, SourceConfig, SourceStats, TextStyle, VideoSource};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// @docs protocol="Text" direction=input
pub struct TextSourceConfig {
    /// Lines of text to display; newlines start a new line.
    pub text: String,
    /// Frame size, colours and alignment.
    #[serde(flatten)]
    pub style: TextStyle,
}

impl Default for TextSourceConfig {
    fn default() -> Self {
        Self {
            text: "Text".to_string(),
            style: TextStyle::default(),
        }
    }
}

/// A rendered text card. Its config can be adopted in place, so a style edit
/// re-renders the frame instead of reopening the source.
pub struct TextSource {
    state: Mutex<TextState>,
    stats: Arc<Mutex<SourceStats>>,
}

struct TextState {
    config: TextSourceConfig,
    frame: Frame,
    seq: u64,
    dirty: bool,
}

impl TextSource {
    pub fn spawn(config: &TextSourceConfig) -> Self {
        let (frame, (width, height)) = render(config, 0);
        let mut stats = SourceStats::new();
        stats.record_frame(width, height, PixelFormat::Rgba8.label(), 0.0);
        return Self {
            state: Mutex::new(TextState {
                config: config.clone(),
                frame,
                seq: 0,
                dirty: false,
            }),
            stats: Arc::new(Mutex::new(stats)),
        };
    }

    /// Re-render the card when `apply_config` marked it dirty.
    fn refresh(&self) {
        let mut state = self.state.lock().unwrap();
        if !state.dirty {
            return;
        }
        state.seq += 1;
        let (frame, (width, height)) = render(&state.config, state.seq);
        self.stats.lock().unwrap().record_frame(width, height, PixelFormat::Rgba8.label(), 0.0);
        state.frame = frame;
        state.dirty = false;
    }
}

impl VideoSource for TextSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        self.refresh();
        return Some(self.state.lock().unwrap().frame.clone());
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }

    fn apply_config(&self, config: &SourceConfig) -> bool {
        let SourceConfig::Text(config) = config else {
            return false;
        };
        let mut state = self.state.lock().unwrap();
        if state.config == *config {
            return true;
        }
        state.config = config.clone();
        state.dirty = true;
        return true;
    }
}

fn render(config: &TextSourceConfig, seq: u64) -> (Frame, (u32, u32)) {
    let style = config.style.clone().bounded();
    let pixels = style.render(&config.text);
    let frame = Frame::Cpu(CpuFrame {
        data: Arc::new(pixels),
        w: style.width,
        h: style.height,
        fmt: PixelFormat::Rgba8,
        pitch: 0,
        seq,
    });
    return (frame, (style.width, style.height));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_size_and_seq(source: &TextSource) -> (u32, u32, u64) {
        let state = source.state.lock().unwrap();
        let Frame::Cpu(frame) = &state.frame else {
            unreachable!("text sources publish CPU frames");
        };
        return (frame.w, frame.h, frame.seq);
    }

    #[test]
    fn the_frame_is_rendered_once_at_the_configured_size() {
        let config = TextSourceConfig::default();
        let source = TextSource::spawn(&config);
        assert_eq!(frame_size_and_seq(&source), (config.style.width, config.style.height, 0));
        let state = source.state.lock().unwrap();
        let Frame::Cpu(frame) = &state.frame else {
            unreachable!("text sources publish CPU frames");
        };
        assert_eq!(frame.fmt, PixelFormat::Rgba8);
        assert_eq!(frame.data.len(), (frame.w * frame.h * 4) as usize);
    }

    #[test]
    fn out_of_range_style_dimensions_are_clamped() {
        let mut config = TextSourceConfig::default();
        config.style.width = 0;
        config.style.height = 10_000;
        config.style.font_size = 0.0;

        let source = TextSource::spawn(&config);

        assert_eq!(frame_size_and_seq(&source).0, 1);
        let stats = source.stats.lock().unwrap();
        assert_eq!((stats.width, stats.height), (1, 4096));
    }

    #[test]
    fn a_changed_config_rerenders_without_reopening() {
        let mut config = TextSourceConfig::default();
        let source = TextSource::spawn(&config);
        assert_eq!(frame_size_and_seq(&source).2, 0);

        config.text = "Changed".to_string();
        config.style.text_color = [255, 0, 0, 255];
        config.style.width = 320;
        assert!(source.apply_config(&SourceConfig::Text(config.clone())));
        assert!(source.state.lock().unwrap().dirty, "an adopted config waits for the next frame");

        source.refresh();
        assert_eq!(frame_size_and_seq(&source).0, 320);
        assert_eq!(frame_size_and_seq(&source).2, 1, "the new picture carries a new seq");
        let stats = source.stats.lock().unwrap();
        assert_eq!((stats.width, stats.height), (320, config.style.height));
    }

    #[test]
    fn an_identical_config_does_not_rerender() {
        let config = TextSourceConfig::default();
        let source = TextSource::spawn(&config);
        assert!(source.apply_config(&SourceConfig::Text(config)));
        assert!(!source.state.lock().unwrap().dirty);
        assert_eq!(frame_size_and_seq(&source).2, 0);
    }
}
