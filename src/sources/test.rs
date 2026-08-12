use super::{CpuFrame, Frame, PixelFormat, VideoSource};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct TestConfig {
    pub width: u32,
    pub height: u32,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self { width: 1280, height: 720 }
    }
}

/// Generated color-bars feed with a moving marker, on its own thread.
pub struct TestSource {
    slot: Arc<Mutex<Option<Frame>>>,
    #[allow(dead_code)]
    name: String,
}

impl TestSource {
    pub fn spawn(name: String, variant: u32) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let writer = slot.clone();
        std::thread::Builder::new()
            .name(format!("src-{name}"))
            .spawn(move || {
                const W: u32 = 1280;
                const H: u32 = 720;
                let mut seq = 0u64;
                loop {
                    let mut buf = vec![0u8; (W * H * 4) as usize];
                    bars(&mut buf, W, H, variant, seq);
                    *writer.lock().unwrap() = Some(Frame::Cpu(CpuFrame {
                        data: Arc::new(buf),
                        w: W,
                        h: H,
                        fmt: PixelFormat::Rgba8,
                        seq,
                    }));
                    seq += 1;
                    std::thread::sleep(Duration::from_millis(33));
                }
            })
            .expect("spawn test source");
        Self { slot, name }
    }
}

impl VideoSource for TestSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        self.slot.lock().unwrap().clone()
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// SMPTE-ish bars, hue-rotated by variant, with a moving white marker column.
fn bars(buf: &mut [u8], w: u32, h: u32, variant: u32, seq: u64) {
    const COLORS: [[u8; 3]; 7] = [
        [192, 192, 192],
        [192, 192, 0],
        [0, 192, 192],
        [0, 192, 0],
        [192, 0, 192],
        [192, 0, 0],
        [0, 0, 192],
    ];
    let marker = ((seq * 8) % w as u64) as u32;
    for y in 0..h {
        for x in 0..w {
            let band = ((x * 7 / w) as usize + variant as usize) % COLORS.len();
            let mut c = COLORS[band];
            if x == marker || y == (seq * 2 % h as u64) as u32 {
                c = [255, 255, 255];
            }
            let i = ((y * w + x) * 4) as usize;
            buf[i] = c[0];
            buf[i + 1] = c[1];
            buf[i + 2] = c[2];
            buf[i + 3] = 255;
        }
    }
}
