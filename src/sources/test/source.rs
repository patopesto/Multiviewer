use super::super::{CpuFrame, Frame, PixelFormat, SourceRef, SourceStats, VideoSource};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};

const NOMINAL_FPS: f64 = 60.0;

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
pub enum ColorSpace {
    Rec601,
    #[default]
    Rec709,
    Rec2020,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
pub enum RadarDirection {
    #[default]
    Right,
    Left,
    Down,
    Up,
}

fn black_color() -> [u8; 3] { [0, 0, 0] }
fn white_color() -> [u8; 3] { [255, 255, 255] }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TestPattern {
    Smpte(SmpteType),
    UvGradient {
        red: bool,
        green: bool,
        blue: bool,
        rotation: f32
    },
    Grid {
        cols: u32,
        rows: u32,
        #[serde(default = "white_color")]
        line_color: [u8; 3],
        #[serde(default = "black_color")]
        bg_color: [u8; 3],
    },
    Radar {
        width: u32,
        speed: f32,
        direction: RadarDirection,
        #[serde(default = "white_color")]
        line_color: [u8; 3],
        #[serde(default = "black_color")]
        bg_color: [u8; 3],
    },
}

impl TestPattern {
    pub const ALL: [TestPattern; 5] = [
        TestPattern::Smpte(SmpteType::Smpte1978),
        TestPattern::Smpte(SmpteType::Smpte2022),
        TestPattern::UvGradient { red: true, green: true, blue: false, rotation: 0.0 },
        TestPattern::Grid { cols: 19, rows: 9, line_color: [255, 255, 255], bg_color: [0, 0, 0] },
        TestPattern::Radar { width: 200, speed: 10.0, direction: RadarDirection::Right, line_color: [255, 255, 255], bg_color: [0, 0, 0] },
    ];

    pub fn label(&self) -> &'static str {
        match self {
            TestPattern::Smpte(SmpteType::Smpte1978) => "SMPTE ECR 1-1978",
            TestPattern::Smpte(SmpteType::Smpte2022) => "SMPTE RP219:2014",
            TestPattern::UvGradient { .. } => "UV Gradient",
            TestPattern::Grid { .. } => "Grid",
            TestPattern::Radar { .. } => "Radar",
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
pub enum SmpteType {
    Smpte1978,
    #[default]
    Smpte2022, // Original spec is RP219:2002 superseded by RP219:2014
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorConfig {
    pub enabled: bool,
    pub speed_x: f32,
    pub speed_y: f32,
    pub width: u32,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            speed_x: 8.0,
            speed_y: 2.0,
            width: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestSourceConfig {
    pub width: u32,
    pub height: u32,
    pub pattern: TestPattern,
    #[serde(default)]
    pub cursor: CursorConfig,
}

impl Default for TestSourceConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            pattern: TestPattern::Smpte(SmpteType::default()),
            cursor: CursorConfig::default(),
        }
    }
}

/// Generated color-bars feed with a moving marker, on its own thread.
pub struct TestSource {
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    source_ref: SourceRef,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestSource {
    pub fn spawn(source_ref: SourceRef, cfg: &TestSourceConfig) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let writer = slot.clone();
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let stats2 = stats.clone();
        let width = cfg.width;
        let height = cfg.height;
        let pattern = cfg.pattern.clone();
        let cursor = cfg.cursor;
        let running = Arc::new(AtomicBool::new(true));
        let running2 = running.clone();
        let thread = std::thread::Builder::new()
            .name(format!("test-in-{source_ref}"))
            .spawn(move || {
                let w = width;
                let h = height;
                let size = (w * h * 4) as usize;
                let frame_interval = Duration::from_secs_f64(1.0 / NOMINAL_FPS);
                let mut seq = 0u64;

                // Render the static base once and reuse it every frame.
                let mut base = vec![0u8; size];
                generate_base(&mut base, w, h, &pattern);
                let base = Arc::new(base);
                let mut back = vec![0u8; size];

                while running2.load(Ordering::Relaxed) {
                    let frame_start = Instant::now();
                    let t0 = Instant::now();
                    match &pattern {
                        TestPattern::Radar { width, speed, direction, line_color, bg_color } => {
                            generate_radar(&mut back, w, h, *width, *speed, seq, direction, *line_color, *bg_color);
                        }
                        _ => {
                            back.copy_from_slice(&base);
                        }
                    }
                    if cursor.enabled {
                        overlay_cursor(&mut back, w, h, seq, &cursor);
                    }
                    let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;
                    {
                        let mut s = stats2.lock().unwrap();
                        s.record_frame(w, h, PixelFormat::Rgba8.label(), NOMINAL_FPS);
                        s.record_copy_time(copy_ms);
                    }
                    let frame = Frame::Cpu(CpuFrame {
                        data: Arc::new(std::mem::take(&mut back)),
                        w,
                        h,
                        fmt: PixelFormat::Rgba8,
                        pitch: 0,
                        seq,
                    });
                    let previous = writer.lock().unwrap().replace(frame);
                    back = match previous {
                        Some(Frame::Cpu(CpuFrame { data, .. })) => {
                            Arc::try_unwrap(data).unwrap_or_else(|_| vec![0u8; size])
                        }
                        _ => vec![0u8; size],
                    };
                    seq += 1;
                    std::thread::sleep(frame_interval.saturating_sub(frame_start.elapsed()));
                }
            })
            .expect("spawn test source");
        Self { slot, stats, source_ref, running, thread: Some(thread) }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take()
            && let Err(e) = t.join()
        {
            tracing::error!(source=self.source_ref, "Thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for TestSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        self.slot.lock().unwrap().clone()
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        self.stats.clone()
    }
}

fn generate_base(buf: &mut [u8], w: u32, h: u32, pattern: &TestPattern) {
    match pattern {
        TestPattern::Smpte(t) => generate_smpte(buf, w, h, t),
        TestPattern::UvGradient { red, green, blue, rotation } => generate_uv_gradient(buf, w, h, *red, *green, *blue, *rotation),
        TestPattern::Grid { cols, rows, line_color, bg_color } => generate_grid(buf, w, h, *cols, *rows, *line_color, *bg_color),
        // Radar is fully dynamic; the base is left black and generated per-frame.
        TestPattern::Radar { .. } => buf.fill(0),
    }
}

/// Overlay an optional moving cursor (white crosshair) on top of any pattern.
fn overlay_cursor(buf: &mut [u8], w: u32, h: u32, seq: u64, cursor: &CursorConfig) {
    if !cursor.enabled || cursor.width == 0 {
        return;
    }
    let x = ((seq as f32 * cursor.speed_x) as u32) % w;
    let y = ((seq as f32 * cursor.speed_y) as u32) % h;
    let x_end = (x + cursor.width).min(w);
    let y_end = (y + cursor.width).min(h);

    // Vertical band.
    for cy in 0..h {
        let row_start = ((cy * w + x) * 4) as usize;
        for cx in x..x_end {
            let i = row_start + ((cx - x) * 4) as usize;
            buf[i] = 255;
            buf[i + 1] = 255;
            buf[i + 2] = 255;
            buf[i + 3] = 255;
        }
    }

    // Horizontal band.
    for cy in y..y_end {
        let row_start = ((cy * w) * 4) as usize;
        for cx in 0..w {
            let i = row_start + (cx * 4) as usize;
            buf[i] = 255;
            buf[i + 1] = 255;
            buf[i + 2] = 255;
            buf[i + 3] = 255;
        }
    }
}

/// Convert narrow-range Y'CbCr directly to narrow-range R'G'B' (studio range).
/// Used by RP219:2014 so that 75% white maps to 180/180/180 and 0% black to 16/16/16.
fn yuv_to_rgb_studio(y: f32, u: f32, v: f32, cs: ColorSpace) -> [u8; 3] {
    let (kr, kb) = match cs {
        ColorSpace::Rec601 => (0.299, 0.114),
        ColorSpace::Rec709 => (0.2126, 0.0722),
        ColorSpace::Rec2020 => (0.2627, 0.0593),
    };
    let kg = 1.0 - kr - kb;
    // Chroma excursion is 224 (16-240) while luma excursion is 219 (16-235).
    // Scale chroma differences to the luma range before applying the matrix.
    let chroma_scale = 219.0 / 224.0;
    let du = (u - 128.0) * chroma_scale;
    let dv = (v - 128.0) * chroma_scale;

    let r = y + 2.0 * (1.0 - kr) * dv;
    let g = y - 2.0 * kb * (1.0 - kb) / kg * du
        - 2.0 * kr * (1.0 - kr) / kg * dv;
    let b = y + 2.0 * (1.0 - kb) * du;

    [
        r.round().clamp(0.0, 255.0) as u8,
        g.round().clamp(0.0, 255.0) as u8,
        b.round().clamp(0.0, 255.0) as u8,
    ]
}

fn generate_smpte(buf: &mut [u8], w: u32, h: u32, t: &SmpteType) {
    match t {
        SmpteType::Smpte1978 => generate_smpte_1978(buf, w, h),
        SmpteType::Smpte2022 => generate_rp2192014(buf, w, h),
    }
}

fn generate_smpte_1978(buf: &mut [u8], w: u32, h: u32) {
    // BT.601 narrow-range Y'CbCr values per SMPTE EG 1-1990 / NTSC color bars.
    let bars_yuv = [
        (180.0, 128.0, 128.0), // 75% White
        (162.0, 44.0, 142.0),  // 75% Yellow
        (131.0, 156.0, 44.0),  // 75% Cyan
        (112.0, 72.0, 58.0),   // 75% Green
        (84.0, 184.0, 198.0),  // 75% Magenta
        (65.0, 100.0, 212.0),  // 75% Red
        (35.0, 212.0, 114.0),  // 75% Blue
    ];
    let cs = ColorSpace::Rec601;

    for y in 0..h {
        let y_f = y as f32 / h as f32;

        for x in 0..w {
            let (y_val, u_val, v_val) = if y_f < 2.0 / 3.0 {
                let band = ((x * 7 / w) as usize) % bars_yuv.len();
                bars_yuv[band]
            } else if y_f < 3.0 / 4.0 {
                let slot = ((x * 7 / w) as usize) % 7;
                match slot {
                    0 => (35.0, 212.0, 114.0),  // 75% Blue
                    2 => (84.0, 184.0, 198.0),  // 75% Magenta
                    4 => (131.0, 156.0, 44.0),  // 75% Cyan
                    6 => (180.0, 128.0, 128.0), // 75% White
                    _ => (16.0, 128.0, 128.0),  // Black
                }
            } else {
                let x_f = x as f32 / w as f32;
                if x_f < 5.0 / 28.0 {
                    (16.0, 158.0, 95.0) // -I (zero-luma)
                } else if x_f < 10.0 / 28.0 {
                    (235.0, 128.0, 128.0) // 100% White
                } else if x_f < 15.0 / 28.0 {
                    (16.0, 174.0, 149.0) // +Q (zero-luma)
                } else if x_f < 5.0 / 7.0 {
                    (16.0, 128.0, 128.0) // Black
                } else if x_f < 6.0 / 7.0 {
                    let pluge_x = (x_f - 5.0 / 7.0) / (1.0 / 7.0);
                    if pluge_x < 1.0 / 3.0 {
                        (7.0, 128.0, 128.0) // Black -4% (-4 IRE)
                    } else if pluge_x < 2.0 / 3.0 {
                        (16.0, 128.0, 128.0) // Black (0 IRE)
                    } else {
                        (24.0, 128.0, 128.0) // Black +4% (+4 IRE)
                    }
                } else {
                    (16.0, 128.0, 128.0) // Black
                }
            };

            let c = yuv_to_rgb_studio(y_val, u_val, v_val, cs);
            let i = ((y * w + x) * 4) as usize;
            buf[i] = c[0];
            buf[i + 1] = c[1];
            buf[i + 2] = c[2];
            buf[i + 3] = 255;
        }
    }
}

/// SMPTE RP219-1:2014 multi-format color bars.
/// Outputs studio-range R'G'B' (16-235) derived from the Annex B Y'CbCr code values.
fn generate_rp2192014(buf: &mut [u8], w: u32, h: u32) {
    let cs = ColorSpace::Rec709;
    // Heights of the four bands (Annex C, Table C.5).
    let h1 = (h * 7) / 12;
    let h2 = h / 12;
    let h3 = h / 12;
    let _h4 = h - h1 - h2 - h3; // bottom 1/3 band

    // Central 4:3 area, leaving 16:9 side extensions.
    let width43 = (w * 3) / 4;
    let side = (w - width43) / 2;
    let left43 = side;
    let right43 = left43 + width43;

    let mut write = |x: u32, y: u32, yuv: (f32, f32, f32)| {
        let c = yuv_to_rgb_studio(yuv.0, yuv.1, yuv.2, cs);
        let i = ((y * w + x) * 4) as usize;
        buf[i] = c[0];
        buf[i + 1] = c[1];
        buf[i + 2] = c[2];
        buf[i + 3] = 255;
    };

    // Pattern 1: 75% color bars inside 4:3, 40% gray side panels.
    let bar_width = (width43 / 7).max(1);
    let bars1 = [
        (180.0, 128.0, 128.0), // 75% White
        (168.0, 44.0, 136.0),  // 75% Yellow
        (145.0, 147.0, 44.0),  // 75% Cyan
        (133.0, 63.0, 52.0),   // 75% Green
        (63.0, 193.0, 204.0),  // 75% Magenta
        (51.0, 109.0, 212.0),  // 75% Red
        (28.0, 212.0, 120.0),  // 75% Blue
    ];
    let gray40 = (104.0, 128.0, 128.0);
    for y in 0..h1 {
        for x in 0..w {
            let yuv = if x < left43 || x >= right43 {
                gray40
            } else {
                let idx = ((x - left43) / bar_width).min(6) as usize;
                bars1[idx]
            };
            write(x, y, yuv);
        }
    }

    // Pattern 2: 100% cyan | 75% white (*2 default also 75% white) | 100% blue.
    let cyan100 = (188.0, 154.0, 16.0);
    let white75 = (180.0, 128.0, 128.0);
    let blue100 = (32.0, 240.0, 118.0);
    for y in h1..(h1 + h2) {
        for x in 0..w {
            // 100% cyan | 75% white (with *2 default also 75% white) | 100% blue.
            let yuv = if x < left43 {
                cyan100
            } else if x < right43 {
                white75
            } else {
                blue100
            };
            write(x, y, yuv);
        }
    }

    // Pattern 3: 100% yellow | *3 default black | Y-ramp | 100% white | 100% red.
    let yellow100 = (219.0, 16.0, 138.0);
    let black0 = (16.0, 128.0, 128.0);
    let white100 = (235.0, 128.0, 128.0);
    let red100 = (63.0, 102.0, 240.0);
    let ramp_left = left43 + width43 / 4;
    let ramp_right = left43 + (width43 * 3) / 4;
    let ramp_width = (ramp_right - ramp_left).max(1);
    for y in (h1 + h2)..(h1 + h2 + h3) {
        for x in 0..w {
            let yuv = if x < left43 {
                yellow100
            } else if x < ramp_left {
                black0
            } else if x < ramp_right {
                let t = (x - ramp_left) as f32 / ramp_width as f32;
                (16.0 + t * (235.0 - 16.0), 128.0, 128.0)
            } else if x < right43 {
                white100
            } else {
                red100
            };
            write(x, y, yuv);
        }
    }

    // Pattern 4: 15% gray side panels | black | white | black | PLUGE | black | 15% gray.
    // Central 4:3 widths are proportional to the 1920x1080 reference in Annex C Table C.2.
    let k = (width43 * 309) / 1440; // 0% black
    let g = (width43 * 411) / 1440; // 100% white
    let hh = (width43 * 171) / 1440; // 0% black
    let i = (width43 * 206) / 1440; // PLUGE -2/0/+2
    let j = (width43 * 137) / 1440; // PLUGE 0/+4
    let m = width43 - k - g - hh - i - j; // 0% black (remainder)

    let xk_end = left43 + k;
    let xg_end = xk_end + g;
    let xh_end = xg_end + hh;
    let xi_end = xh_end + i;
    let xj_end = xi_end + j;
    let xm_end = xj_end + m; // == right43

    let gray15 = (49.0, 128.0, 128.0);
    for y in (h1 + h2 + h3)..h {
        for x in 0..w {
            let y_val = if x < left43 || x >= right43 {
                gray15.0
            } else if x < xk_end {
                16.0
            } else if x < xg_end {
                235.0
            } else if x < xh_end {
                16.0
            } else if x < xi_end {
                let sub = x - xh_end;
                let part = (i / 3).max(1);
                if sub < part {
                    12.0 // -2% black
                } else if sub < 2 * part {
                    16.0 // 0% black
                } else {
                    20.0 // +2% black
                }
            } else if x < xj_end {
                let sub = x - xi_end;
                let part = (j / 2).max(1);
                if sub < part {
                    16.0 // 0% black
                } else {
                    25.0 // +4% black
                }
            } else if x < xm_end {
                16.0
            } else {
                gray15.0
            };
            write(x, y, (y_val, 128.0, 128.0));
        }
    }
}

fn generate_uv_gradient(buf: &mut [u8], w: u32, h: u32, r: bool, g: bool, b: bool, rotation_deg: f32) {
    let theta = rotation_deg.to_radians();
    let cos = theta.cos();
    let sin = theta.sin();
    for y in 0..h {
        let ny = y as f32 / h as f32 - 0.5;
        for x in 0..w {
            let nx = x as f32 / w as f32 - 0.5;
            // Rotate the sampling coordinates around the image center.
            let u = nx * cos - ny * sin + 0.5;
            let v = nx * sin + ny * cos + 0.5;
            let i = ((y * w + x) * 4) as usize;
            buf[i] = if r { (u * 255.0).clamp(0.0, 255.0) as u8 } else { 0 };
            buf[i + 1] = if g { (v * 255.0).clamp(0.0, 255.0) as u8 } else { 0 };
            buf[i + 2] = if b { ((1.0 - u) * 255.0).clamp(0.0, 255.0) as u8 } else { 0 };
            buf[i + 3] = 255;
        }
    }
}

fn generate_grid(buf: &mut [u8], w: u32, h: u32, cols: u32, rows: u32, line_color: [u8; 3], bg_color: [u8; 3]) {
    let cw = w / cols;
    let rh = h / rows;
    for y in 0..h {
        for x in 0..w {
            let is_line = (x % cw < 2) || (y % rh < 2);
            let c = if is_line { line_color } else { bg_color };
            let i = ((y * w + x) * 4) as usize;
            buf[i] = c[0];
            buf[i + 1] = c[1];
            buf[i + 2] = c[2];
            buf[i + 3] = 255;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_radar(buf: &mut [u8], w: u32, h: u32, width: u32, speed: f32, seq: u64, direction: &RadarDirection, line_color: [u8; 3], bg_color: [u8; 3]) {
    let (dim_main, is_horizontal, reverse) = match direction {
        RadarDirection::Right => (w, true, false),
        RadarDirection::Left => (w, true, true),
        RadarDirection::Down => (h, false, false),
        RadarDirection::Up => (h, false, true),
    };

    let cycle = dim_main as f32 + width as f32;
    let pos = (seq as f32 * speed) % cycle;
    let edge_pos = pos as i32;

    for y in 0..h {
        for x in 0..w {
            let raw_coord = if is_horizontal { x as i32 } else { y as i32 };
            let coord = if reverse {
                (dim_main - 1) as i32 - raw_coord
            } else {
                raw_coord
            };

            let intensity = if coord == edge_pos {
                1.0
            } else if coord < edge_pos {
                let dist = (coord - (edge_pos - width as i32)) as f32;
                (dist / width as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };

            let i = ((y * w + x) * 4) as usize;
            buf[i] = lerp_u8(bg_color[0], line_color[0], intensity);
            buf[i + 1] = lerp_u8(bg_color[1], line_color[1], intensity);
            buf[i + 2] = lerp_u8(bg_color[2], line_color[2], intensity);
            buf[i + 3] = 255;
        }
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (a as f32 + (b as f32 - a as f32) * t) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn studio_converter_maps_gray_levels_exactly() {
        // 0% black -> 16, 75% white -> 180, 100% white -> 235.
        assert_eq!(yuv_to_rgb_studio(16.0, 128.0, 128.0, ColorSpace::Rec709), [16, 16, 16]);
        assert_eq!(yuv_to_rgb_studio(180.0, 128.0, 128.0, ColorSpace::Rec709), [180, 180, 180]);
        assert_eq!(yuv_to_rgb_studio(235.0, 128.0, 128.0, ColorSpace::Rec709), [235, 235, 235]);
    }

    #[test]
    fn rp2192014_pattern1_bars_and_side_panels() {
        let w = 1920;
        let h = 1080;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        generate_rp2192014(&mut buf, w, h);

        // 75% white bar (central 4:3 area, first bar).
        assert_eq!(pixel(&buf, w, 300, 100), [180, 180, 180, 255]);
        // 40% gray side panel.
        assert_eq!(pixel(&buf, w, 100, 100), [104, 104, 104, 255]);
    }

    #[test]
    fn rp2192014_pattern2_extensions() {
        let w = 1920;
        let h = 1080;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        generate_rp2192014(&mut buf, w, h);

        let h1 = (h * 7) / 12;
        let y = h1 + 10;
        let c = pixel(&buf, w, 100, y);
        // 100% cyan: red low, green/blue high.
        assert!(c[0] < 32);
        assert!(c[1] > 220);
        assert!(c[2] > 220);

        // 75% white in the 4:3 left half.
        assert_eq!(pixel(&buf, w, 600, y), [180, 180, 180, 255]);

        // 100% blue right extension: blue high, red/green low.
        let b = pixel(&buf, w, 1700, y);
        assert!(b[2] > 220);
        assert!(b[0] < 32);
        assert!(b[1] < 32);
    }

    #[test]
    fn rp2192014_pattern3_ramp_and_extensions() {
        let w = 1920;
        let h = 1080;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        generate_rp2192014(&mut buf, w, h);

        let y = (h * 7) / 12 + h / 12 + 10;

        // 100% yellow left extension: red/green high, blue low.
        let yel = pixel(&buf, w, 100, y);
        assert!(yel[0] > 220);
        assert!(yel[1] > 220);
        assert!(yel[2] < 32);

        // Ramp starts at black and ends at white.
        assert_eq!(pixel(&buf, w, 600, y), [16, 16, 16, 255]);
        assert_eq!(pixel(&buf, w, 1319, y), [235, 235, 235, 255]);

        // 100% red right extension: red high, green/blue low.
        let r = pixel(&buf, w, 1700, y);
        assert!(r[0] > 220);
        assert!(r[1] < 32);
        assert!(r[2] < 32);
    }

    #[test]
    fn rp2192014_pattern4_pluge_and_gray() {
        let w = 1920;
        let h = 1080;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        generate_rp2192014(&mut buf, w, h);

        let y = (h * 7) / 12 + h / 6 + 10;

        // 15% gray side panel.
        assert_eq!(pixel(&buf, w, 100, y), [49, 49, 49, 255]);

        // 100% white bar.
        assert_eq!(pixel(&buf, w, 750, y), [235, 235, 235, 255]);

        // PLUGE -2% black (Y=12) and +4% black (Y=25).
        assert_eq!(pixel(&buf, w, 1150, y), [12, 12, 12, 255]);
        assert_eq!(pixel(&buf, w, 1450, y), [25, 25, 25, 255]);
    }

    #[test]
    fn smpte1978_75_white_is_studio_range() {
        let w = 1280;
        let h = 720;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        generate_smpte_1978(&mut buf, w, h);
        // 75% white maps to studio-range 180/180/180.
        assert_eq!(pixel(&buf, w, 10, 10), [180, 180, 180, 255]);
    }

    #[test]
    fn smpte1978_bottom_iq_and_pluge() {
        let w = 1280;
        let h = 720;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        generate_smpte_1978(&mut buf, w, h);

        let y = h * 3 / 4 + 10; // bottom band

        // -I is a dark blue/cyan: low red, moderate green, high blue.
        let i = pixel(&buf, w, (w as f32 * 2.5 / 28.0) as u32, y);
        assert!(i[0] < i[2], "-I should be blue-ish: {:?}", i);
        assert!(i[0] < 10, "-I red should be near zero: {:?}", i);

        // +Q is a dark magenta: red/blue high, green low.
        let q = pixel(&buf, w, (w as f32 * 12.5 / 28.0) as u32, y);
        assert!(q[1] < q[0] && q[1] < q[2], "+Q should be magenta-ish: {:?}", q);
        assert!(q[1] < 10, "+Q green should be near zero: {:?}", q);

        // 100% white bar.
        let white = pixel(&buf, w, (w as f32 * 7.5 / 28.0) as u32, y);
        assert_eq!(white, [235, 235, 235, 255]);

        // PLUGE -4 IRE (Y=7).
        let pluge_neg = pixel(&buf, w, (w as f32 * (5.0 / 7.0 + 1.0 / 21.0)) as u32, y);
        assert_eq!(pluge_neg, [7, 7, 7, 255]);
    }
}
