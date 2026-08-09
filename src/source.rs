use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    // Phase 2: Uyvy (NDI). Phase 3: Gpu texture frames (Syphon).
}

#[derive(Clone)]
pub struct CpuFrame {
    pub data: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
    #[allow(dead_code)]
    pub fmt: PixelFormat,
    /// Monotonic per-source counter; compositor uploads only when this changes.
    pub seq: u64,
}

#[derive(Clone)]
pub enum Frame {
    Cpu(CpuFrame),
}

pub trait VideoSource: Send + Sync {
    /// Latest frame, non-blocking. None if nothing received yet.
    fn latest(&self) -> Option<Frame>;
    fn name(&self) -> &str;
}

/// All live sources. Owned by the UI thread; sources render their own threads.
pub struct Registry {
    sources: Vec<Box<dyn VideoSource>>,
    next_test: u32,
}

impl Registry {
    pub fn new() -> Self {
        Self { sources: Vec::new(), next_test: 0 }
    }

    pub fn add_test(&mut self) {
        self.next_test += 1;
        let letter = (b'A' + (self.next_test as u8 - 1) % 26) as char;
        let name = format!("Test {letter}");
        let src = TestSource::spawn(name, self.next_test);
        self.sources.push(Box::new(src));
    }

    pub fn add_ndi(&mut self, name: String, source: grafton_ndi::Source) {
        if self.sources.iter().any(|s| s.name() == name) {
            return; // already connected
        }
        let src = crate::ndi::NdiSource::spawn(name.clone(), source);
        self.sources.push(Box::new(src));
    }

    pub fn get(&self, name: &str) -> Option<&dyn VideoSource> {
        self.sources.iter().find(|s| s.name() == name).map(|s| &**s)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.sources.iter().map(|s| s.name())
    }

    #[allow(dead_code)]
    pub fn remove(&mut self, name: &str) {
        self.sources.retain(|s| s.name() != name);
    }
}

/// Generated color-bars feed with a moving marker, on its own thread.
pub struct TestSource {
    slot: Arc<Mutex<Option<Frame>>>,
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
    fn latest(&self) -> Option<Frame> {
        // ponytail: lock+clone of Arc per frame per cell is cheap enough at 16 sources
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
