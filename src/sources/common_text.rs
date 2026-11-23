use std::sync::OnceLock;

use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle as GlyphStyle};
use serde::{Deserialize, Serialize};

/// Horizontal placement of a text block inside its frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

impl TextAlign {
    pub fn label(&self) -> &'static str {
        return match self {
            TextAlign::Left => "Left",
            TextAlign::Center => "Center",
            TextAlign::Right => "Right",
        };
    }
}

/// Frame and text appearance shared by the text-rendering sources.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    /// Rasterized frame width in pixels.
    pub width: u32,
    /// Rasterized frame height in pixels.
    pub height: u32,
    /// Text height in pixels.
    pub font_size: f32,
    /// Text colour, as RGBA.
    pub text_color: [u8; 4],
    /// Background colour, as RGBA. A transparent background lets the text sit over other sources.
    pub background_color: [u8; 4],
    /// Horizontal alignment of the text block.
    pub align: TextAlign,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            font_size: 72.0,
            text_color: [255, 255, 255, 255],
            background_color: [0, 0, 0, 0],
            align: TextAlign::Center,
        }
    }
}

impl TextStyle {
    /// Largest raster the settings and a project file may ask for, in pixels.
    pub const MAX_SIZE: (u32, u32) = (4096, 4096);

    /// Bounds the raster size a project file may carry.
    pub fn bounded(self) -> Self {
        let (max_width, max_height) = Self::MAX_SIZE;
        return Self {
            width: self.width.clamp(1, max_width),
            height: self.height.clamp(1, max_height),
            font_size: self.font_size.clamp(1.0, 1024.0),
            ..self
        };
    }

    /// Render `text` into a fresh RGBA frame.
    pub fn render(&self, text: &str) -> Vec<u8> {
        return render_text_frame(self.width, self.height, text, self.font_size, self.text_color, self.background_color, self.align);
    }
}

/// Embedded font, parsed once for every text block rasterized in the process.
fn font() -> &'static fontdue::Font {
    static FONT: OnceLock<fontdue::Font> = OnceLock::new();
    return FONT.get_or_init(|| {
        fontdue::Font::from_bytes(epaint_default_fonts::HACK_REGULAR as &[u8], fontdue::FontSettings::default()).expect("embedded font is valid")
    });
}

/// Rasterize `text` into a tight straight-alpha RGBA block at `size` pixels.
pub fn rasterize_text(text: &str, size: f32, color: [u8; 4]) -> Option<(u32, u32, Vec<u8>)> {
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings::default());
    layout.append(&[font()], &GlyphStyle::new(text, size.max(1.0), 0));

    let glyphs = layout.glyphs();
    if glyphs.is_empty() {
        return None;
    }

    let max_x = glyphs.iter().map(|g| g.x + g.width as f32).fold(0.0_f32, f32::max).ceil() as i32;
    let min_y = glyphs.iter().map(|g| g.y.floor() as i32).min().unwrap_or(0);
    let max_y = glyphs.iter().map(|g| (g.y + g.height as f32).ceil() as i32).max().unwrap_or(0);
    let width = max_x.max(1) as u32;
    let height = (max_y - min_y).max(1) as u32;

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let [tr, tg, tb, ta] = color;
    let font = font();
    for glyph in glyphs {
        if !glyph.char_data.rasterize() {
            continue;
        }
        let (metrics, coverage) = font.rasterize_config(glyph.key);
        let gx = glyph.x as u32;
        let gy = (glyph.y as i32 - min_y) as u32;
        for row in 0..metrics.height as u32 {
            for col in 0..metrics.width as u32 {
                let dst_x = gx + col;
                let dst_y = gy + row;
                if dst_x >= width || dst_y >= height {
                    continue;
                }
                let cov = coverage[(row * metrics.width as u32 + col) as usize];
                let alpha = ((cov as u32 * ta as u32) / 255) as u8;
                if alpha == 0 {
                    continue;
                }
                // Glyphs of one run do not overlap, so the coverage writes over.
                let i = ((dst_y * width + dst_x) * 4) as usize;
                pixels[i] = tr;
                pixels[i + 1] = tg;
                pixels[i + 2] = tb;
                pixels[i + 3] = alpha;
            }
        }
    }
    return Some((width, height, pixels));
}

/// Render `text` into a fresh frame, aligned, over `background`.
fn render_text_frame(width: u32, height: u32, text: &str, size: f32, color: [u8; 4], background: [u8; 4], align: TextAlign) -> Vec<u8> {
    let width = width.max(1);
    let height = height.max(1);
    let mut frame = vec![0u8; (width as usize) * (height as usize) * 4];
    for pixel in frame.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&background);
    }

    let Some((block_w, block_h, block)) = rasterize_text(text, size, color) else {
        return frame;
    };
    let (src_x, x0) = align_offsets(align, block_w, width);
    let visible_w = block_w.min(width);
    let visible_h = block_h.min(height);
    let y0 = (height - visible_h) / 2;

    for row in 0..visible_h {
        for col in 0..visible_w {
            let src = ((row * block_w + src_x + col) * 4) as usize;
            let src_a = block[src + 3] as u32;
            if src_a == 0 {
                continue;
            }
            let dst = (((y0 + row) * width + x0 + col) * 4) as usize;
            let dst_a = frame[dst + 3] as u32;
            let out_a = src_a + (dst_a * (255 - src_a)) / 255;
            for channel in 0..3 {
                let src_c = block[src + channel] as u32;
                let dst_c = frame[dst + channel] as u32;
                frame[dst + channel] = ((src_c * src_a * 255 + dst_c * dst_a * (255 - src_a)) / (255 * out_a)) as u8;
            }
            frame[dst + 3] = out_a as u8;
        }
    }
    return frame;
}

/// Where the visible part of a block starts in the block and in the frame.
fn align_offsets(align: TextAlign, block_w: u32, width: u32) -> (u32, u32) {
    let visible = block_w.min(width);
    return match align {
        TextAlign::Left => (0, 0),
        TextAlign::Center => ((block_w - visible) / 2, (width - visible) / 2),
        TextAlign::Right => (block_w - visible, width - visible),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        return frame[i..i + 4].try_into().unwrap();
    }

    #[test]
    fn empty_text_renders_only_the_background() {
        let frame = render_text_frame(16, 8, "", 8.0, [255, 0, 0, 255], [0, 0, 255, 255], TextAlign::Left);
        assert_eq!(frame.len(), 16 * 8 * 4);
        assert!(frame.as_chunks::<4>().0.iter().all(|p| p == &[0, 0, 255, 255]));
    }

    #[test]
    fn text_is_drawn_over_an_opaque_background() {
        let frame = render_text_frame(128, 64, "M", 48.0, [255, 255, 255, 255], [0, 0, 0, 255], TextAlign::Center);
        let lit = (0..64)
            .flat_map(|y| (0..128).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&frame, 128, x, y)[..3].iter().any(|c| *c > 128))
            .count();
        assert!(lit > 0, "no text pixels drawn");
        assert_eq!(pixel(&frame, 128, 0, 0), [0, 0, 0, 255]);
    }

    /// A transparent background leaves unlit pixels transparent, so a text
    /// source can sit over video without a box around it.
    #[test]
    fn transparent_background_stays_transparent() {
        let frame = render_text_frame(128, 64, "M", 48.0, [255, 255, 255, 255], [0, 0, 0, 0], TextAlign::Center);
        assert_eq!(pixel(&frame, 128, 0, 0), [0, 0, 0, 0]);
        assert!(frame.as_chunks::<4>().0.iter().any(|p| p[3] > 0), "no text pixels drawn");
        // Text is straight alpha: a lit pixel keeps the full glyph colour.
        let lit = frame.as_chunks::<4>().0.iter().find(|p| p[3] == 255).expect("solid glyph pixel");
        assert_eq!(lit, &[255, 255, 255, 255]);
    }

    #[test]
    fn alignment_moves_the_block() {
        let left = render_text_frame(256, 64, "II", 32.0, [255, 255, 255, 255], [0, 0, 0, 0], TextAlign::Left);
        let right = render_text_frame(256, 64, "II", 32.0, [255, 255, 255, 255], [0, 0, 0, 0], TextAlign::Right);
        let lit_x = |frame: &[u8]| {
            return (0..256).find(|&x| (0..64).any(|y| pixel(frame, 256, x, y)[3] > 0)).unwrap();
        };
        assert!(lit_x(&right) > lit_x(&left));
    }

    /// A block wider than the frame is cropped from the side the alignment
    /// anchors, not always from the left.
    #[test]
    fn alignment_crops_an_oversized_block_from_the_correct_side() {
        assert_eq!(align_offsets(TextAlign::Left, 200, 100), (0, 0));
        assert_eq!(align_offsets(TextAlign::Center, 200, 100), (50, 0));
        assert_eq!(align_offsets(TextAlign::Right, 200, 100), (100, 0));

        assert_eq!(align_offsets(TextAlign::Left, 40, 100), (0, 0));
        assert_eq!(align_offsets(TextAlign::Center, 40, 100), (0, 30));
        assert_eq!(align_offsets(TextAlign::Right, 40, 100), (0, 60));
    }

    /// The frame stays straight alpha whatever its background, because the
    /// compositor multiplies by alpha again when it draws the source.
    #[test]
    fn translucent_text_keeps_its_colour_over_a_transparent_background() {
        let frame = render_text_frame(128, 64, "M", 48.0, [255, 255, 255, 128], [0, 0, 0, 0], TextAlign::Center);
        let lit = frame.as_chunks::<4>().0.iter().find(|p| p[3] == 128).expect("covered glyph pixel");
        assert_eq!(lit, &[255, 255, 255, 128]);
    }

    #[test]
    fn text_over_a_translucent_background_is_straight_alpha() {
        let frame = render_text_frame(128, 64, "M", 48.0, [255, 255, 255, 255], [0, 0, 0, 128], TextAlign::Center);
        assert_eq!(pixel(&frame, 128, 0, 0), [0, 0, 0, 128]);
        let lit = frame.as_chunks::<4>().0.iter().find(|p| p[3] == 255).expect("covered glyph pixel");
        assert_eq!(lit, &[255, 255, 255, 255]);
    }

    /// A newline starts a line but is not itself drawn as a glyph.
    #[test]
    fn a_newline_adds_a_line_without_drawing_a_glyph() {
        let one = rasterize_text("M", 48.0, [255, 255, 255, 255]).unwrap();
        let two = rasterize_text("M\nM", 48.0, [255, 255, 255, 255]).unwrap();
        let lit = |block: &[u8]| block.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
        assert!(two.1 > one.1, "the newline did not start a second line");
        assert_eq!(lit(&two.2), 2 * lit(&one.2), "the newline drew ink");
    }
}
