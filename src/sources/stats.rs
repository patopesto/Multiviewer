use std::collections::VecDeque;
use std::time::{Duration, Instant};

const FPS_WINDOW: Duration = Duration::from_secs(2);
const TIMING_WINDOW: usize = 60;
const MAX_FRAME_TIMESTAMPS: usize = 256;

/// Push a timestamp, evict samples older than the FPS window, and cap length.
fn push_and_trim(instants: &mut VecDeque<Instant>, now: Instant) {
    instants.push_back(now);
    while let Some(front) = instants.front() {
        if now.duration_since(*front) > FPS_WINDOW {
            instants.pop_front();
        } else {
            break;
        }
    }
    if instants.len() > MAX_FRAME_TIMESTAMPS {
        instants.pop_front();
    }
}

fn fps_of(instants: &VecDeque<Instant>) -> f64 {
    if instants.len() < 2 {
        return 0.0;
    }
    let secs = instants
        .back()
        .unwrap()
        .duration_since(*instants.front().unwrap())
        .as_secs_f64();
    if secs <= 0.0 {
        return 0.0;
    }
    return (instants.len() - 1) as f64 / secs;
}

#[derive(Debug, Clone)]
pub struct SourceStats {
    pub width: u32,
    pub height: u32,
    pub pixel_format: String,
    pub nominal_fps: f64,
    pub computed_fps: f64,
    pub frames_received: u64,
    pub frames_dropped: u64,
    pub frames_consumed: u64, /// Distinct frames the compositor has seen; the gap to `frames_received` is what nobody pulled.
    pub receive_time_ms: f32, /// Main-thread cost of `latest()`
    pub copy_time_ms: f32,
    pub upload_time_ms: f32,
    pub upload_mbps: f32, /// CPU->GPU bandwidth over the FPS window; 0 for GPU-direct sources.
    pub off_screen: bool, // For screencapturekit
    recent_frames: VecDeque<Instant>,
    receive_times: VecDeque<f32>,
    copy_times: VecDeque<f32>,
    upload_times: VecDeque<f32>,
    upload_samples: VecDeque<(Instant, u64)>,
    last_consumed_seq: Option<u64>,
}

impl Default for SourceStats {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            pixel_format: String::new(),
            nominal_fps: 0.0,
            computed_fps: 0.0,
            frames_received: 0,
            frames_dropped: 0,
            frames_consumed: 0,
            receive_time_ms: 0.0,
            copy_time_ms: 0.0,
            upload_time_ms: 0.0,
            upload_mbps: 0.0,
            off_screen: false,
            recent_frames: VecDeque::new(),
            receive_times: VecDeque::new(),
            copy_times: VecDeque::new(),
            upload_times: VecDeque::new(),
            upload_samples: VecDeque::new(),
            last_consumed_seq: None,
        }
    }
}

impl SourceStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_frame(&mut self, width: u32, height: u32, pixel_format: &str, nominal_fps: f64) {
        let now = Instant::now();
        self.width = width;
        self.height = height;
        self.pixel_format = pixel_format.to_string();
        self.nominal_fps = nominal_fps;
        self.frames_received += 1;
        push_and_trim(&mut self.recent_frames, now);
        self.computed_fps = fps_of(&self.recent_frames);
    }

    pub fn record_copy_time(&mut self, ms: f32) {
        self.copy_times.push_back(ms);
        if self.copy_times.len() > TIMING_WINDOW {
            self.copy_times.pop_front();
        }
        self.copy_time_ms = self.average(&self.copy_times);
    }

    pub fn record_receive_time(&mut self, ms: f32) {
        self.receive_times.push_back(ms);
        if self.receive_times.len() > TIMING_WINDOW {
            self.receive_times.pop_front();
        }
        self.receive_time_ms = self.average(&self.receive_times);
    }

    /// Count a frame the compositor observed with a new seq number.
    pub fn record_consumed(&mut self, seq: u64) {
        if self.last_consumed_seq == Some(seq) {
            return;
        }
        if let Some(last) = self.last_consumed_seq
            && seq > last + 1
        {
            self.frames_dropped += seq - last - 1;
        }
        self.last_consumed_seq = Some(seq);
        self.frames_consumed += 1;
    }

    pub fn record_upload_time(&mut self, ms: f32, bytes: u64) {
        self.upload_times.push_back(ms);
        if self.upload_times.len() > TIMING_WINDOW {
            self.upload_times.pop_front();
        }
        self.upload_time_ms = self.average(&self.upload_times);

        let now = Instant::now();
        self.upload_samples.push_back((now, bytes));
        while let Some(&(t, _)) = self.upload_samples.front() {
            if now.duration_since(t) > FPS_WINDOW {
                self.upload_samples.pop_front();
            } else {
                break;
            }
        }
        if self.upload_samples.len() < 2 {
            return;
        }
        let span = now
            .duration_since(self.upload_samples.front().unwrap().0)
            .as_secs_f32();
        let total: u64 = self.upload_samples.iter().map(|(_, b)| *b).sum();
        self.upload_mbps = if span > 0.0 {
            total as f32 / 1_000_000.0 / span
        } else {
            0.0
        };
    }

    #[cfg(target_os = "macos")]
    pub fn set_off_screen(&mut self, off_screen: bool) {
        self.off_screen = off_screen;
    }

    fn average(&self, values: &VecDeque<f32>) -> f32 {
        if values.is_empty() {
            return 0.0;
        }
        values.iter().sum::<f32>() / values.len() as f32
    }
}

#[derive(Debug, Clone, Default)]
pub struct OutputStats {
    pub width: u32,
    pub height: u32,
    pub computed_fps: f64,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub send_time_ms: f32,
    send_instants: VecDeque<Instant>,
}

impl OutputStats {
    /// Count a frame handed to the consumer and refresh the send rate.
    pub fn record_sent(&mut self) {
        self.frames_sent += 1;
        push_and_trim(&mut self.send_instants, Instant::now());
        self.computed_fps = fps_of(&self.send_instants);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_records_and_averages() {
        let mut s = SourceStats::new();
        s.record_frame(1920, 1080, "BGRA8", 30.0);
        s.record_copy_time(1.0);
        s.record_copy_time(3.0);
        s.record_receive_time(0.5);
        s.record_receive_time(1.5);
        s.record_upload_time(2.0, 3_000_000);
        s.record_upload_time(4.0, 3_000_000);
        s.record_consumed(0);
        s.record_consumed(2);
        assert_eq!(s.width, 1920);
        assert_eq!(s.height, 1080);
        assert_eq!(s.pixel_format, "BGRA8");
        assert!((s.nominal_fps - 30.0).abs() < 0.001);
        assert_eq!(s.frames_received, 1);
        assert_eq!(s.frames_dropped, 1);
        assert!((s.copy_time_ms - 2.0).abs() < 0.001);
        assert!((s.receive_time_ms - 1.0).abs() < 0.001);
        assert!((s.upload_time_ms - 3.0).abs() < 0.001);
        assert!(s.upload_mbps > 0.0);
    }

    /// Only new seqs count as consumed; a source re-observed between frames
    /// must not inflate the figure.
    #[test]
    fn stats_counts_consumed_once_per_seq() {
        let mut s = SourceStats::new();
        s.record_consumed(0);
        s.record_consumed(0);
        s.record_consumed(1);
        s.record_consumed(1);
        s.record_consumed(2);
        assert_eq!(s.frames_consumed, 3);
    }

    /// A seq jump is frames produced but never displayed; same or lower seq
    /// (double pull, source restart) must not count.
    #[test]
    fn stats_counts_seq_gaps_as_dropped() {
        let mut s = SourceStats::new();
        s.record_consumed(0);
        s.record_consumed(4);
        assert_eq!(s.frames_dropped, 3);
        s.record_consumed(4);
        s.record_consumed(2);
        assert_eq!(s.frames_dropped, 3);
        assert_eq!(s.frames_consumed, 3);
    }

    /// A single sample is not enough to measure a rate; the second frame
    /// produces a finite non-zero send rate.
    #[test]
    fn output_stats_records_send_rate() {
        let mut s = OutputStats::default();
        s.record_sent();
        assert_eq!(s.frames_sent, 1);
        assert_eq!(s.computed_fps, 0.0);
        s.record_sent();
        assert_eq!(s.frames_sent, 2);
        assert!(s.computed_fps > 0.0);
    }
}
