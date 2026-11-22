use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::super::{CpuFrame, Frame, PixelFormat, SourceStats, TextStyle, VideoSource};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// @docs protocol="Text" direction=input
pub struct TextSourceConfig {
    /// Lines of text to display; newlines start a new line.
    pub text: String,
    /// Frame size, colours and alignment.
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

/// A rendered text card. Its config is fixed when the source opens, so the
/// frame is rasterized once and every placement of it shares that frame.
pub struct TextSource {
    frame: Frame,
    stats: Arc<Mutex<SourceStats>>,
}

impl TextSource {
    pub fn spawn(config: &TextSourceConfig) -> Self {
        // A project file can carry sizes the UI cannot produce; bound them once
        // so the frame and its pixels agree.
        let mut style = config.style.clone();
        style.width = style.width.clamp(1, 4096);
        style.height = style.height.clamp(1, 4096);
        style.font_size = style.font_size.clamp(1.0, 1024.0);

        let pixels = style.render(&config.text);
        let mut stats = SourceStats::new();
        stats.record_frame(style.width, style.height, PixelFormat::Rgba8.label(), 0.0);
        return Self {
            frame: Frame::Cpu(CpuFrame {
                data: Arc::new(pixels),
                w: style.width,
                h: style.height,
                fmt: PixelFormat::Rgba8,
                pitch: 0,
                seq: 0,
            }),
            stats: Arc::new(Mutex::new(stats)),
        };
    }
}

impl VideoSource for TextSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        return Some(self.frame.clone());
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        return self.stats.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_is_rendered_once_at_the_configured_size() {
        let config = TextSourceConfig::default();
        let source = TextSource::spawn(&config);
        let Frame::Cpu(frame) = &source.frame else {
            unreachable!("text sources publish CPU frames");
        };
        assert_eq!((frame.w, frame.h), (config.style.width, config.style.height));
        assert_eq!(frame.fmt, PixelFormat::Rgba8);
        assert_eq!(frame.seq, 0);
        assert_eq!(frame.data.len(), (frame.w * frame.h * 4) as usize);
    }

    #[test]
    fn out_of_range_style_dimensions_are_clamped() {
        let mut config = TextSourceConfig::default();
        config.style.width = 0;
        config.style.height = 10_000;
        config.style.font_size = 0.0;

        let source = TextSource::spawn(&config);

        let Frame::Cpu(frame) = &source.frame else {
            unreachable!("text sources publish CPU frames");
        };
        assert_eq!((frame.w, frame.h), (1, 4096));
        assert_eq!(frame.data.len(), (frame.w * frame.h * 4) as usize);
        let stats = source.stats.lock().unwrap();
        assert_eq!((stats.width, stats.height), (1, 4096));
    }
}
