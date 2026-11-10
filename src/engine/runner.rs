use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::compositor::{Draw, Rect, Shared};
use crate::sources::Protocol;

use super::{Command, Engine, EngineSnapshot};

const PREVIEW_FPS: f64 = 60.0;
const OUTPUT_FPS: f64 = 60.0;
const CONTROL_FPS: f64 = 60.0;
const MAINTENANCE_INTERVAL: Duration = Duration::from_millis(250);
const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(1);
const FRAME_TOLERANCE: Duration = Duration::from_millis(2);
const RENDER_FPS_WINDOW: Duration = Duration::from_secs(2);
const MAX_RENDER_TIMESTAMPS: usize = 256;
const UI_STALL: Duration = Duration::from_millis(250);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(250);

/// One preview frame built by the runner and painted by the UI.
#[derive(Clone)]
pub struct PreviewFrame {
    pub rect: Rect,
    pub shared: Arc<Shared>,
    pub draw: Arc<Draw>,
}

struct RunnerState {
    visible: bool,
    last_ui_tick: Instant,
    preview_rect: Rect,
    discovery_focus: Option<Protocol>,
}

/// Ticks the engine at a fixed rate and publishes the latest preview frame.
pub struct Runner {
    state: Arc<Mutex<RunnerState>>,
    preview: Arc<Mutex<Option<PreviewFrame>>>,
    snapshot: Arc<Mutex<Option<Arc<EngineSnapshot>>>>,
    commands: mpsc::Sender<Command>,
    stats: Arc<Mutex<RunnerStats>>,
    stop: Arc<AtomicBool>,
    done: Receiver<()>,
}

impl Runner {
    pub fn start(
        engine: Arc<Mutex<Engine>>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        preview_enabled: bool,
    ) -> Option<Self> {
        let state = Arc::new(Mutex::new(RunnerState {
            visible: false,
            last_ui_tick: Instant::now(),
            preview_rect: Rect::default(),
            discovery_focus: None,
        }));
        let preview = Arc::new(Mutex::new(None));
        let snapshot = Arc::new(Mutex::new(None));
        let (commands, command_rx) = mpsc::channel();
        let stats = Arc::new(Mutex::new(RunnerStats::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = mpsc::channel();
        let thread_state = state.clone();
        let thread_preview = preview.clone();
        let thread_snapshot = snapshot.clone();
        let thread_stats = stats.clone();
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("engine-runner".into())
            .spawn(move || {
                run(
                    engine,
                    device,
                    queue,
                    format,
                    thread_state,
                    thread_preview,
                    thread_snapshot,
                    command_rx,
                    thread_stats,
                    thread_stop,
                    preview_enabled,
                );
                let _ = done_tx.send(());
            })
            .ok()?;
        return Some(Self {
            state,
            preview,
            snapshot,
            commands,
            stats,
            stop,
            done,
        });
    }

    /// Update the window visibility from the paint loop.
    pub fn set_window_state(&self, visible: bool) {
        let mut state = self.state.lock().unwrap();
        state.visible = visible;
        state.last_ui_tick = Instant::now();
    }

    /// The panel rect the preview is built against.
    pub fn set_preview_rect(&self, rect: Rect) {
        self.state.lock().unwrap().preview_rect = rect;
    }

    /// The protocol whose discovered sources the UI is currently showing.
    pub fn set_discovery_focus(&self, focus: Option<&Protocol>) {
        let mut state = self.state.lock().unwrap();
        if state.discovery_focus.as_ref() == focus {
            return;
        }
        state.discovery_focus = focus.cloned();
    }

    pub fn preview(&self) -> Option<PreviewFrame> {
        return self.preview.lock().unwrap().clone();
    }

    pub fn snapshot(&self) -> Option<Arc<EngineSnapshot>> {
        return self.snapshot.lock().unwrap().clone();
    }

    pub fn commands(&self) -> mpsc::Sender<Command> {
        return self.commands.clone();
    }

    pub fn stats(&self) -> Arc<Mutex<RunnerStats>> {
        return self.stats.clone();
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Bounded: the thread can sit inside a blocking output present, in which
        // case it detaches and the process exit takes the engine down with it.
        let _ = self.done.recv_timeout(SHUTDOWN_GRACE);
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Debug)]
pub enum StartupError {
    Missing(String),
    Open(String),
}

impl StartupError {
    pub fn message(&self) -> String {
        return match self {
            StartupError::Missing(message) => message.clone(),
            StartupError::Open(message) => message.clone(),
        };
    }
}

/// Engine and runner created from the same startup path, shared by the native
/// window and `--headless`.
pub struct EngineRuntime {
    pub engine: Arc<Mutex<Engine>>,
    pub runner: Runner,
    pub startup_error: Option<StartupError>,
    pub startup_warning: Option<String>,
}

impl EngineRuntime {
    pub fn start(
        startup_path: Option<PathBuf>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        preview_enabled: bool,
    ) -> Result<Self, String> {
        let mut engine = Engine::new_project();
        let mut startup_error = None;
        let mut startup_warning = None;
        if let Some(path) = startup_path {
            if path.exists() {
                match engine.open_project(&path) {
                    Ok(()) => {
                        if !engine.load_warnings.is_empty() {
                            startup_warning = Some(engine.load_warnings.join(" "));
                        }
                    }
                    Err(e) => startup_error = Some(StartupError::Open(e.to_string())),
                }
            } else {
                startup_error = Some(StartupError::Missing(format!(
                    "Startup project not found: {}",
                    path.display()
                )));
            }
        }

        let engine = Arc::new(Mutex::new(engine));
        let runner = Runner::start(engine.clone(), device, queue, format, preview_enabled)
            .ok_or_else(|| "spawn engine runner".to_string())?;
        return Ok(Self {
            engine,
            runner,
            startup_error,
            startup_warning,
        });
    }
}

/// Moving-window tick statistics shared with the status bar.
pub struct RunnerStats {
    recent_frames: VecDeque<Instant>,
    work_ms: VecDeque<f32>,
    gpu_ms: Option<f32>,
}

impl RunnerStats {
    pub fn new() -> Self {
        return Self {
            recent_frames: VecDeque::new(),
            work_ms: VecDeque::new(),
            gpu_ms: None,
        };
    }

    pub fn record_frame(&mut self) {
        let now = Instant::now();
        self.recent_frames.push_back(now);
        while let Some(front) = self.recent_frames.front() {
            if now.duration_since(*front) > RENDER_FPS_WINDOW {
                self.recent_frames.pop_front();
            } else {
                break;
            }
        }
        if self.recent_frames.len() > MAX_RENDER_TIMESTAMPS {
            self.recent_frames.pop_front();
        }
    }

    pub fn record_work(&mut self, elapsed: Duration) {
        self.work_ms.push_back(elapsed.as_secs_f32() * 1000.0);
        if self.work_ms.len() > MAX_RENDER_TIMESTAMPS {
            self.work_ms.pop_front();
        }
    }

    pub fn record_gpu(&mut self, gpu_ms: Option<f32>) {
        self.gpu_ms = gpu_ms;
    }

    pub fn work_ms(&self) -> f32 {
        if self.work_ms.is_empty() {
            return 0.0;
        }
        return self.work_ms.iter().sum::<f32>() / self.work_ms.len() as f32;
    }

    pub fn gpu_ms(&self) -> Option<f32> {
        return self.gpu_ms;
    }

    pub fn fps(&self) -> f64 {
        if self.recent_frames.len() < 2 {
            return 0.0;
        }
        let front = self.recent_frames.front().unwrap();
        let back = self.recent_frames.back().unwrap();
        let secs = back.duration_since(*front).as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        return (self.recent_frames.len() - 1) as f64 / secs;
    }
}

fn advance_deadline(prev: Instant, now: Instant, interval: Duration) -> Instant {
    return if now >= prev + interval {
        now + interval
    } else {
        prev + interval
    };
}

fn publish_if_changed(
    engine: &Engine,
    snapshot: &Mutex<Option<Arc<EngineSnapshot>>>,
    published_revision: &mut u64,
) {
    if engine.state.revision == *published_revision {
        return;
    }
    publish_snapshot(engine, snapshot, false, None);
    *published_revision = engine.state.revision;
}

fn publish_snapshot(
    engine: &Engine,
    snapshot: &Mutex<Option<Arc<EngineSnapshot>>>,
    refresh_runtime: bool,
    focus: Option<&Protocol>,
) {
    let mut slot = snapshot.lock().unwrap();
    if refresh_runtime {
        *slot = Some(Arc::new(engine.snapshot_focused(focus)));
        return;
    }
    let runtime = slot
        .as_ref()
        .map(|snapshot| snapshot.runtime.clone())
        .unwrap_or_default();
    *slot = Some(Arc::new(EngineSnapshot {
        state: engine.state.clone(),
        runtime,
    }));
}

#[allow(clippy::too_many_arguments)]
fn run(
    engine_arc: Arc<Mutex<Engine>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    state: Arc<Mutex<RunnerState>>,
    preview: Arc<Mutex<Option<PreviewFrame>>>,
    snapshot: Arc<Mutex<Option<Arc<EngineSnapshot>>>>,
    command_rx: Receiver<Command>,
    stats: Arc<Mutex<RunnerStats>>,
    stop: Arc<AtomicBool>,
    preview_enabled: bool,
) {
    let mut next_maintenance = Instant::now();
    let mut next_auto_save = Instant::now();
    let mut next_control = Instant::now();
    let mut next_preview = Instant::now();
    let mut next_output = Instant::now();
    let mut published_revision;

    {
        let engine = engine_arc.lock().unwrap();
        publish_snapshot(&engine, &snapshot, true, None);
        published_revision = engine.state.revision;
    }

    while !stop.load(Ordering::Relaxed) {
        let _span = tracing::debug_span!("tick").entered();
        let (visible, preview_rect, last_ui_tick, discovery_focus) = {
            let state = state.lock().unwrap();
            (
                state.visible,
                state.preview_rect,
                state.last_ui_tick,
                state.discovery_focus.clone(),
            )
        };
        let painting = preview_enabled && visible && last_ui_tick.elapsed() <= UI_STALL;

        let mut engine = engine_arc.lock().unwrap();
        engine.canvas_rect = preview_rect;
        while let Ok(command) = command_rx.try_recv() {
            if let Err(e) = engine.apply_command(command) {
                tracing::warn!("engine command rejected: {e}");
            }
        }
        publish_if_changed(&engine, &snapshot, &mut published_revision);
        let outputs = engine.output_registry.any_enabled();
        let control = engine.control.is_enabled();
        let now = Instant::now();

        let mut wake = next_maintenance.min(next_auto_save);
        if control {
            wake = wake.min(next_control);
        }
        if painting {
            wake = wake.min(next_preview);
        }
        if outputs {
            wake = wake.min(next_output);
        }
        if now + FRAME_TOLERANCE < wake {
            drop(engine);
            match command_rx.recv_timeout(wake - now) {
                Ok(command) => {
                    let latest_rect = state.lock().unwrap().preview_rect;
                    let mut engine = engine_arc.lock().unwrap();
                    engine.canvas_rect = latest_rect;
                    if let Err(e) = engine.apply_command(command) {
                        tracing::warn!("engine command rejected: {e}");
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {}
            }
            continue;
        }

        let started = Instant::now();

        if now + FRAME_TOLERANCE >= next_maintenance {
            next_maintenance = advance_deadline(next_maintenance, now, MAINTENANCE_INTERVAL);
            engine.maintain();
            publish_snapshot(&engine, &snapshot, true, discovery_focus.as_ref());
            published_revision = engine.state.revision;
        }
        if now + FRAME_TOLERANCE >= next_auto_save {
            next_auto_save = advance_deadline(next_auto_save, now, AUTO_SAVE_INTERVAL);
            engine.auto_save();
        }
        if control && now + FRAME_TOLERANCE >= next_control {
            next_control = advance_deadline(
                next_control,
                now,
                Duration::from_secs_f64(1.0 / CONTROL_FPS),
            );
            engine.poll_control();
            publish_if_changed(&engine, &snapshot, &mut published_revision);
        }

        let outputs = engine.output_registry.any_enabled();
        let preview_due = painting && now + FRAME_TOLERANCE >= next_preview;
        let output_due = outputs && now + FRAME_TOLERANCE >= next_output;
        let mut new_preview = None;
        let mut gpu_tick = false;
        let mut gpu_ms = None;

        if preview_due || output_due {
            engine.ensure_compositor(&device, &queue, format);
            engine.begin_tick();

            if preview_due {
                next_preview = advance_deadline(
                    next_preview,
                    now,
                    Duration::from_secs_f64(1.0 / PREVIEW_FPS),
                );
                engine.prefetch_preview(&device, &queue);
                if preview_rect.width() > 0.0 && preview_rect.height() > 0.0 {
                    let transform = super::transform::display_transform(
                        &engine.state.view,
                        &engine.state.cfg.canvas,
                        &preview_rect,
                        engine.state.ui_visible,
                    );
                    let draw = engine.build_frame(&device, &queue, &preview_rect, transform);
                    if let Some(shared) = engine.shared() {
                        new_preview = Some(PreviewFrame {
                            rect: preview_rect,
                            shared,
                            draw: Arc::new(draw),
                        });
                    }
                }
            }

            if output_due {
                next_output = advance_deadline(
                    next_output,
                    now,
                    Duration::from_secs_f64(1.0 / OUTPUT_FPS),
                );
                engine.render_outputs(&device, &queue);
            }

            gpu_ms = engine.finish_tick(started.elapsed().as_secs_f32() * 1000.0);
            gpu_tick = true;
        }

        drop(engine);

        if let Some(frame) = new_preview {
            *preview.lock().unwrap() = Some(frame);
        }

        let elapsed = started.elapsed();
        let mut stats = stats.lock().unwrap();
        stats.record_frame();
        stats.record_work(elapsed);
        if gpu_tick {
            stats.record_gpu(gpu_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Canvas;

    #[test]
    fn publishes_when_the_revision_changes() {
        let mut engine = Engine::test_instance(Canvas::default());
        let snapshot = Mutex::new(None);
        let mut published_revision = engine.state.revision;

        publish_if_changed(&engine, &snapshot, &mut published_revision);
        assert!(snapshot.lock().unwrap().is_none());

        engine.state.revision = engine.state.revision.wrapping_add(1);
        publish_if_changed(&engine, &snapshot, &mut published_revision);
        assert!(snapshot.lock().unwrap().is_some());
    }

    #[test]
    fn deadline_advances_on_a_fixed_grid_when_running_early() {
        let interval = Duration::from_millis(16);
        let deadline = Instant::now() + interval;
        let now = deadline - Duration::from_millis(2);
        assert_eq!(advance_deadline(deadline, now, interval), deadline + interval);
    }

    #[test]
    fn deadline_resyncs_when_fallen_behind() {
        let interval = Duration::from_millis(16);
        let deadline = Instant::now();
        let now = deadline + Duration::from_millis(100);
        assert_eq!(advance_deadline(deadline, now, interval), now + interval);
    }
}
