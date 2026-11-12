use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::super::{APP_NAME, PROJECT_FILE_EXTENSION};
use crate::engine::{
    Command, Engine, EngineRuntime, RepaintWake, Runner, RunnerStats, ViewerId, publish_state,
};
use crate::platform::poll_open_file;
use crate::ui::remote::RemoteHost;
use crate::ui::viewer::{self, HostAction, HostView, UiMode, Viewer, ensure_extension};

const VISIBLE_REPAINT: Duration = Duration::from_millis(16);
const HIDDEN_REPAINT: Duration = Duration::from_secs(1);
/// Waiting this long for the engine lock means the tick thread is busy in a way
/// the user experiences as a frozen window.
const SLOW_LOCK: Duration = Duration::from_millis(100);

pub struct App {
    engine: Arc<Mutex<Engine>>,
    runner: Runner,
    commands: mpsc::Sender<Command>,
    viewer: Viewer,
    remote: RemoteHost,
    present_stats: RunnerStats,
    /// Set by `--exit-after`; the first frame past it closes the window so
    /// `on_exit` runs and the trace flushes.
    deadline: Option<Instant>,
    trace_guard: Option<tracing_chrome::FlushGuard>,
}

// Init stuff
impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        startup_path: Option<PathBuf>,
        trace_guard: Option<tracing_chrome::FlushGuard>,
        deadline: Option<Instant>,
    ) -> Self {
        let (gpu_backend, gpu_name) = match &cc.wgpu_render_state {
            Some(render_state) => {
                let info = render_state.adapter.get_info();
                (Some(format!("{:?}", info.backend)), Some(info.name))
            }
            None => (None, None),
        };
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .expect("eframe wgpu render state");
        let runtime = EngineRuntime::start(
            startup_path,
            render_state.device.clone(),
            render_state.queue.clone(),
            render_state.target_format,
            true,
            Some(repaint_wake(&cc.egui_ctx)),
        )
        .expect("spawn engine runner");
        let engine = runtime.engine;
        let runner = runtime.runner;
        let mut viewer = Viewer::new(UiMode::Native);
        viewer.gpu_backend = gpu_backend;
        viewer.gpu_name = gpu_name;
        viewer.last_error = runtime.startup_error.map(|error| error.message());
        viewer.last_warning = runtime.startup_warning;

        let remote = RemoteHost::new(
            engine.clone(),
            runner.snapshot_slot(),
            runner.commands(),
            runner.focus_handle(ViewerId::Remote),
        );

        super::configure(&cc.egui_ctx);

        let commands = runner.commands();
        return Self {
            engine,
            runner,
            commands,
            viewer,
            remote,
            present_stats: RunnerStats::new(),
            deadline,
            trace_guard,
        };
    }

    fn execute_actions(&mut self, actions: Vec<HostAction>) {
        let Some(snapshot) = self.runner.snapshot() else {
            return;
        };
        let mut actions = actions;
        let mut index = 0;
        while index < actions.len() {
            let action = actions[index].clone();
            match action {
                HostAction::OpenDialog => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter(format!("{} project", APP_NAME), &[PROJECT_FILE_EXTENSION])
                        .pick_file()
                    {
                        self.viewer.request_open(path, &snapshot, &mut actions);
                    }
                }
                HostAction::SaveAsDialog => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter(format!("{} project", APP_NAME), &[PROJECT_FILE_EXTENSION])
                        .set_file_name(format!("Untitled.{PROJECT_FILE_EXTENSION}"))
                        .save_file()
                    {
                        let path = ensure_extension(path, PROJECT_FILE_EXTENSION);
                        self.run_action(&HostAction::SaveAsPath(path));
                    }
                }
                HostAction::NewProject | HostAction::OpenPath(_) | HostAction::SaveAsPath(_) => {
                    self.run_action(&action);
                }
            }
            index += 1;
        }
    }

    fn run_action(&mut self, action: &HostAction) {
        let engine = self.engine.clone();
        let snapshots = self.runner.snapshot_slot();
        let mut engine = lock_engine(&engine);
        let result = self.viewer.run_action(&mut engine, action);
        if let Err(e) = result {
            self.viewer.last_error = Some(e);
        }
        publish_state(&engine, &snapshots);
    }

    fn request_open(&mut self, path: PathBuf) {
        let Some(snapshot) = self.runner.snapshot() else {
            return;
        };
        let mut actions = Vec::new();
        self.viewer.request_open(path, &snapshot, &mut actions);
        self.execute_actions(actions);
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
        for file in dropped {
            let path = file.path().to_path_buf();
            if path.extension().and_then(|extension| extension.to_str())
                == Some(PROJECT_FILE_EXTENSION)
            {
                self.request_open(path);
                break;
            }
        }
    }

    fn handle_platform_open_files(&mut self) {
        if let Some(path) = poll_open_file()
            && path.extension().and_then(|extension| extension.to_str())
                == Some(PROJECT_FILE_EXTENSION)
        {
            self.request_open(path);
        }
    }

    fn update_title(&self, ctx: &egui::Context) {
        let Some(snapshot) = self.runner.snapshot() else {
            return;
        };
        let filename = snapshot
            .state
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());
        let prefix = if snapshot.state.dirty { "* " } else { "" };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{prefix}{filename} — {APP_NAME}"
        )));
    }
}

impl eframe::App for App {
    // Called once per frame before ui(), should not perform any drawing here
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            self.deadline = None;
            tracing::info!("run deadline reached, closing");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        self.handle_dropped_files(ctx);
        self.handle_platform_open_files();
        self.update_title(ctx);
        self.remote.sync();

        let (minimized, occluded) = ctx.input(|input| {
            let viewport = input.viewport();
            (viewport.minimized.unwrap_or(false), viewport.occluded)
        });
        // Minimised or fully occluded means nothing is showing the preview.
        // Focus is deliberately not part of this: a partially covered window
        // is still visible and its preview must keep running.
        let visible = !minimized && occluded != Some(true);
        self.runner.set_window_state(visible);

        let interval = if visible {
            VISIBLE_REPAINT
        } else {
            HIDDEN_REPAINT
        };
        ctx.request_repaint_after(interval);
    }

    // Draw function
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let Some(snapshot) = self.runner.snapshot() else {
            return;
        };
        self.present_stats.record_frame();

        let discovery_focus = viewer::selected_protocol(&snapshot);
        self.runner
            .set_discovery_focus(ViewerId::Native, discovery_focus.as_ref());

        let (tick_fps, work_ms, gpu_ms) = {
            let stats = self.runner.stats();
            let stats = stats.lock().unwrap();
            (stats.fps(), stats.work_ms(), stats.gpu_ms())
        };
        let present_fps = self.present_stats.fps();
        let mut status = format!("{tick_fps:.0} tick · {present_fps:.0} present · {work_ms:.1} ms");
        let mut tooltip = String::from(
            "Engine ticks over the last 2s · UI frames presented over the last 2s · CPU time per tick",
        );
        if let Some(gpu_ms) = gpu_ms {
            status.push_str(&format!(" · {gpu_ms:.1} ms GPU"));
            tooltip.push_str(" · GPU canvas time");
        }

        let preview = self.runner.preview();
        let remote_status = self.remote.status_label();
        let output = self.viewer.draw(
            ui,
            &snapshot,
            &self.commands,
            HostView {
                preview: preview.as_ref(),
                focus: None,
                status: &status,
                status_tooltip: &tooltip,
                web_status: remote_status.as_deref(),
            },
        );
        self.runner.set_preview_rect(output.canvas_rect);
        self.execute_actions(output.actions);
    }

    fn on_exit(&mut self) {
        let started = Instant::now();
        tracing::info!("exiting");
        self.remote.stop();
        self.runner.stop();
        let engine = self.engine.clone();
        lock_engine(&engine).log_run_totals();
        drop(self.trace_guard.take());
        tracing::info!(total_ms = started.elapsed().as_millis() as u64, "exit");
    }
}

fn repaint_wake(ctx: &egui::Context) -> RepaintWake {
    let ctx = ctx.clone();
    return Arc::new(move || ctx.request_repaint());
}

fn lock_engine(engine: &Arc<Mutex<Engine>>) -> MutexGuard<'_, Engine> {
    let started = Instant::now();
    let engine = engine.lock().unwrap();
    let waited = started.elapsed();
    if waited >= SLOW_LOCK {
        // The tick thread holds this lock across discovery, output presents
        // and any inline source teardown, so a long wait is the UI freeze.
        tracing::warn!(waited_ms = waited.as_millis() as u64, "engine lock wait");
    }
    return engine;
}
