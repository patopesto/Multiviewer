use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::super::{APP_NAME, PROJECT_FILE_EXTENSION};
use crate::control::{Command, ControlStatus, FeedbackMode, Selector};
use crate::engine::{Engine, EngineRuntime, Runner, RunnerStats};
use crate::platform::poll_open_file;
use crate::session::Session;
use crate::ui::canvas;
use crate::ui::menu_bar::{self, MenuAction};
use crate::ui::side_panel;
use crate::ui::shortcuts::{self, Shortcut};

const DEFAULT_PROJECT_NAME: &str = "Untitled";
const LICENSES_TEXT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/LICENSE.md"));

const VISIBLE_REPAINT: Duration = Duration::from_millis(16);
const HIDDEN_REPAINT: Duration = Duration::from_secs(1);
/// Waiting this long for the engine lock means the tick thread is busy in a way
/// the user experiences as a frozen window.
const SLOW_LOCK: Duration = Duration::from_millis(100);

pub struct App {
    engine: Arc<Mutex<Engine>>,
    runner: Runner,
    commands: mpsc::Sender<Command>,
    canvas_ui: canvas::CanvasUi,
    panel: side_panel::SidePanel,
    present_stats: RunnerStats,
    recent_projects: Vec<PathBuf>,
    last_error: Option<String>,
    last_warning: Option<String>,
    pending_confirm: Option<Confirm>,
    show_about: bool,
    show_shortcuts: bool,
    show_control: bool,
    show_licenses: bool,
    licenses_cache: CommonMarkCache,
    vendor_versions: Vec<(&'static str, Option<String>)>,
    gpu_backend: Option<String>,
    gpu_name: Option<String>,
    /// Set by `--exit-after`; the first frame past it closes the window so
    /// `on_exit` runs and the trace flushes.
    deadline: Option<Instant>,
    trace_guard: Option<tracing_chrome::FlushGuard>,
}

enum Confirm {
    New,
    Open,
    OpenPath(PathBuf),
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
        )
        .expect("spawn engine runner");
        let engine = runtime.engine;
        let runner = runtime.runner;
        let last_error = runtime.startup_error.map(|error| error.message());
        let last_warning = runtime.startup_warning;

        Self::configure_egui(&cc.egui_ctx);

        let commands = runner.commands();
        let mut app = Self {
            engine,
            runner,
            commands,
            canvas_ui: canvas::CanvasUi::default(),
            panel: side_panel::SidePanel::default(),
            present_stats: RunnerStats::new(),
            recent_projects: Vec::new(),
            last_error,
            last_warning,
            pending_confirm: None,
            show_about: false,
            show_shortcuts: false,
            show_control: false,
            show_licenses: false,
            licenses_cache: CommonMarkCache::default(),
            vendor_versions: Engine::vendor_versions(),
            gpu_backend,
            gpu_name,
            deadline,
            trace_guard,
        };
        app.refresh_recent();
        return app;
    }

    fn engine(&self) -> MutexGuard<'_, Engine> {
        let started = Instant::now();
        let engine = self.engine.lock().unwrap();
        let waited = started.elapsed();
        if waited >= SLOW_LOCK {
            // The tick thread holds this lock across discovery, output presents
            // and any inline source teardown, so a long wait is the UI freeze.
            tracing::warn!(waited_ms = waited.as_millis() as u64, "engine lock wait");
        }
        return engine;
    }

    fn configure_egui(ctx: &egui::Context) {
        // Lock to dark theme
        ctx.set_theme(egui::ThemePreference::Dark);

        // Enable image/SVG loading for asset icons
        egui_extras::install_image_loaders(ctx);
    
        // Add "Hack" font to render glyphs (→, ←, ↓, ↑, etc..)
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("Hack".to_owned());
        ctx.set_fonts(fonts);
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

        let (minimized, occluded) = ctx.input(|i| {
            let viewport = i.viewport();
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
        let ctx = ui.ctx().clone();
        self.present_stats.record_frame();

        let snapshot = self.runner.snapshot();
        let ui_visible = snapshot
            .as_ref()
            .map(|snapshot| snapshot.state.ui_visible)
            .unwrap_or(true);
        let actions = if ui_visible {
            match snapshot.as_deref() {
                Some(snapshot) => {
                    let discovery_focus =
                        snapshot
                            .state
                            .selected_source_id
                            .as_ref()
                            .and_then(|id| {
                                snapshot
                                    .state
                                    .cfg
                                    .canvas
                                    .sources
                                    .iter()
                                    .find(|source| &source.uuid == id)
                                    .map(|source| &source.protocol)
                            });
                    self.runner.set_discovery_focus(discovery_focus);
                    let actions = menu_bar::draw(ui, snapshot, &self.recent_projects);
                    side_panel::draw(ui, snapshot, &mut self.panel, &self.commands);
                    actions
                }
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        if !actions.is_empty() {
            self.handle_menu_actions(&actions, ui);
        }
        self.handle_global_shortcuts(ui);
        self.draw_confirmation_modal(&ctx);
        self.draw_about_modal(&ctx);
        self.draw_shortcuts_modal(&ctx);
        self.draw_control_window(&ctx);
        self.draw_licenses_modal(&ctx);
        if ui_visible {
            self.draw_status_bar(ui);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(snapshot) = &snapshot {
                canvas::update(
                    ui,
                    snapshot,
                    &self.commands,
                    &mut self.canvas_ui,
                    &self.runner,
                    &ctx,
                );
            }
        });
    }

    fn on_exit(&mut self) {
        let started = Instant::now();
        tracing::info!("exiting");
        self.runner.stop();
        self.engine().log_run_totals();
        drop(self.trace_guard.take());
        tracing::info!(total_ms = started.elapsed().as_millis() as u64, "exit");
    }
}

// Update stuff
impl App {
    /// Promote warnings collected while loading the current project to the status bar.
    fn refresh_warning(&mut self) {
        let warning = {
            let engine = self.engine();
            if engine.load_warnings.is_empty() {
                None
            } else {
                Some(engine.load_warnings.join(" "))
            }
        };
        self.last_warning = warning;
    }

    fn open_path(&mut self, path: &std::path::Path) {
        let result = self.engine().open_project(path);
        if let Err(e) = result {
            self.last_error = Some(e.to_string());
        } else {
            self.last_error = None;
            self.refresh_warning();
            self.refresh_recent();
        }
    }

    fn refresh_recent(&mut self) {
        self.recent_projects = Session::load().recent_projects;
    }

    fn confirm_or(&mut self, action: Confirm) {
        let dirty = self.engine().state.dirty;
        if dirty {
            self.pending_confirm = Some(action);
        } else {
            self.execute_confirm(action);
        }
    }

    fn execute_confirm(&mut self, action: Confirm) {
        match action {
            Confirm::New => {
                *self.engine() = Engine::new_project();
                self.panel = side_panel::SidePanel::default();
                self.canvas_ui = canvas::CanvasUi::default();
                self.last_error = None;
                self.last_warning = None;
            }
            Confirm::Open => self.open_dialog(),
            Confirm::OpenPath(path) => self.open_path(&path),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(format!("{} project", APP_NAME), &[PROJECT_FILE_EXTENSION])
            .pick_file()
        {
            self.open_path(&path);
        }
    }

    fn save_as_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(format!("{} project", APP_NAME), &[PROJECT_FILE_EXTENSION])
            .set_file_name(format!("{}.{}", DEFAULT_PROJECT_NAME, PROJECT_FILE_EXTENSION))
            .save_file()
        {
            let path = ensure_extension(path, PROJECT_FILE_EXTENSION);
            let result = self.engine().save_project_as(&path);
            if let Err(e) = result {
                self.last_error = Some(e.to_string());
            } else {
                self.last_error = None;
                self.refresh_recent();
            }
        }
    }

    fn handle_menu_actions(&mut self, actions: &[MenuAction], ui: &egui::Ui) {
        if self.pending_confirm.is_some() {
            return;
        }
        for action in actions {
            match action {
                MenuAction::Shortcut(shortcut) => self.apply_shortcut(*shortcut, ui),
                MenuAction::OpenRecent(path) => self.confirm_or(Confirm::OpenPath(path.clone())),
                MenuAction::SetAutoSaveInterval(secs) => self.run_command(Command::SetAutoSaveInterval(*secs)),
                MenuAction::ShowAbout => self.show_about = true,
                MenuAction::ShowShortcuts => self.show_shortcuts = true,
                MenuAction::ShowOSCControl => self.show_control = true,
                MenuAction::ShowLicenses => self.show_licenses = true,
            }
        }
    }

    fn handle_global_shortcuts(&mut self, ui: &egui::Ui) {
        let ctx = ui.ctx();
        // Don't fire shortcuts while a modal is open or while typing in a text field.
        if self.pending_confirm.is_some() || ctx.text_edit_focused() {
            return;
        }
        let Some(shortcut) = Shortcut::detect_global(ctx) else {
            return;
        };

        // In hidden UI mode only allow toggling back and exiting expanded view.
        let ui_visible = self
            .runner
            .snapshot()
            .map(|snapshot| snapshot.state.ui_visible)
            .unwrap_or(true);
        if !ui_visible {
            match shortcut {
                Shortcut::ToggleUi | Shortcut::ExitExpanded => {}
                _ => return,
            }
        }

        self.apply_shortcut(shortcut, ui);
    }

    /// Run one shortcut. Shared by keyboard detection and the menu bar so both paths behave identically.
    fn apply_shortcut(&mut self, shortcut: Shortcut, ui: &egui::Ui) {
        // UI-only actions keep their dialogs and confirmation; everything else
        // goes through the same command path as the control protocols.
        match shortcut {
            Shortcut::NewProject => {
                self.confirm_or(Confirm::New);
                return;
            }
            Shortcut::OpenProject => {
                self.confirm_or(Confirm::Open);
                return;
            }
            Shortcut::SaveProjectAs => {
                self.save_as_dialog();
                return;
            }
            Shortcut::SaveProject => {
                if self.engine().state.project_path.is_some() {
                    let result = self.engine().apply_command(Command::SaveProject);
                    if let Err(e) = result {
                        self.last_error = Some(e.to_string());
                    } else {
                        self.last_error = None;
                    }
                } else {
                    self.save_as_dialog();
                }
                return;
            }
            _ => {}
        }

        let nudge_amount = if ui.ctx().input(|i| i.modifiers.alt) { 10.0 } else { 1.0 };
        let Some(command) = shortcut_command(shortcut, &self.engine(), nudge_amount) else {
            return;
        };
        self.run_command(command);
    }

    fn run_command(&mut self, command: Command) {
        let result = self.engine().apply_command(command);
        if let Err(e) = result {
            self.last_error = Some(e.to_string());
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for file in dropped {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) == Some(PROJECT_FILE_EXTENSION) {
                self.confirm_or(Confirm::Open);
                break;
            }
        }
    }

    fn handle_platform_open_files(&mut self) {
        if let Some(path) = poll_open_file()
            && path.extension().and_then(|e| e.to_str()) == Some(PROJECT_FILE_EXTENSION)
        {
            self.confirm_or(Confirm::OpenPath(path));
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
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| DEFAULT_PROJECT_NAME.to_string());
        let prefix = if snapshot.state.dirty { "* " } else { "" };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{}{} — {}", prefix, filename, APP_NAME)));
    }

    fn draw_confirmation_modal(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_confirm.as_ref() else { return };
        let title = match pending {
            Confirm::New => "New project",
            Confirm::Open => "Open project",
            Confirm::OpenPath(_) => "Open project",
        };
        let mut action: Option<Confirm> = None;
        let response = egui::Modal::new(egui::Id::new("confirm_modal"))
            .backdrop_color(egui::Color32::from_black_alpha(175))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(egui::RichText::new(title).heading().strong());
                ui.add_space(12.0);
                ui.label("You have unsaved changes. Discard them?");
                ui.add_space(20.0);
                ui.separator();
                egui::Sides::new().show(ui, |_ui| {}, |ui| { 
                    if ui.add(egui::Button::new("Discard").fill(egui::Color32::from_rgb(140, 30, 30))).clicked() {
                        action = self.pending_confirm.take();
                    }
                    if ui.button("Cancel").clicked() {
                        self.pending_confirm = None;
                    }
                });
        });
        if response.should_close() {
            self.pending_confirm = None;
        }
        if let Some(action) = action {
            self.execute_confirm(action);
        }
    }

    /// Project status shown in the bottom status bar.
    fn status_text(&self) -> String {
        if let Some(err) = &self.last_error {
            return format!("Error: {err}");
        }
        if let Some(warn) = &self.last_warning {
            return format!("Warning: {warn}");
        }
        let Some(snapshot) = self.runner.snapshot() else {
            return "Saved".to_string();
        };
        if snapshot.state.dirty {
            if snapshot.state.project_path.is_some() {
                return "Unsaved changes".to_string();
            }
            return "Untitled — use Save As".to_string();
        }
        return "Saved".to_string();
    }

    fn draw_status_bar(&self, ui: &mut egui::Ui) {
        let (tick_fps, work_ms, gpu_ms) = {
            let stats = self.runner.stats();
            let stats = stats.lock().unwrap();
            (stats.fps(), stats.work_ms(), stats.gpu_ms())
        };
        let present_fps = self.present_stats.fps();
        let mut text = format!("{tick_fps:.0} tick · {present_fps:.0} present · {work_ms:.1} ms");
        let mut label = String::from(
            "Engine ticks over the last 2s · UI frames presented over the last 2s · CPU time per tick",
        );
        if let Some(gpu_ms) = gpu_ms {
            text.push_str(&format!(" · {gpu_ms:.1} ms GPU"));
            label.push_str(" · GPU canvas time");
        }
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.status_text());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(text).on_hover_text(label);
                });
            });
        });
    }

    fn draw_about_modal(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut open_licenses = false;
        let open = centered_window(ctx, format!("About {}", APP_NAME), false, None, |ui| {
            ui.heading(APP_NAME);
            ui.label(env!("CARGO_PKG_DESCRIPTION"));
            let copyright = env!("APP_COPYRIGHT");
            let authors = env!("CARGO_PKG_AUTHORS").replace(':', ", ");
            let credit = if !copyright.is_empty() && (authors.is_empty() || copyright.contains(authors.as_str())) {
                copyright.to_string()
            } else if !copyright.is_empty() {
                format!("{copyright} · {authors}")
            } else if !authors.is_empty() {
                format!("© {} {authors}", env!("BUILD_DATE").get(..4).unwrap_or(""))
            } else {
                String::new()
            };
            if !credit.is_empty() {
                ui.label(credit);
            }
            ui.horizontal(|ui| {
                let license = format!("{} License", env!("CARGO_PKG_LICENSE"));
                let license_url = format!("{}/-/blob/master/LICENSE", env!("CARGO_PKG_REPOSITORY"));
                if !license.is_empty() {
                    ui.hyperlink_to(license, license_url);
                }
                let homepage = env!("CARGO_PKG_HOMEPAGE");
                if !homepage.is_empty() {
                    ui.hyperlink_to("Website", homepage);
                }
                let repository = env!("CARGO_PKG_REPOSITORY");
                if !repository.is_empty() {
                    ui.hyperlink_to("Repository", repository);
                }
            });
            ui.separator();

            egui::Grid::new("about_metadata").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                ui.label("Version");
                ui.label(env!("CARGO_PKG_VERSION"));
                ui.end_row();
                ui.label("Commit");
                ui.label(env!("GIT_COMMIT"));
                ui.end_row();
                ui.label("Build Date");
                ui.label(env!("BUILD_DATE"));
                ui.end_row();

                ui.label("Renderer");
                ui.label(match (&self.gpu_backend, &self.gpu_name) {
                    (Some(backend), Some(name)) => format!("{backend} · {name}"),
                    _ => "unavailable".to_string(),
                });
                ui.end_row();
            });

            ui.separator();

            egui::Grid::new("about_versions").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                for (name, version) in &self.vendor_versions {
                    ui.label(*name);
                    ui.label(version.as_deref().unwrap_or("unavailable"));
                    ui.end_row();
                }
            });
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("NDI® is a registered trademark of Vizrt NDI AB.");
                ui.hyperlink_to("ndi.video", "https://ndi.video/");
            });
            if ui.button("Third-Party Licenses…").clicked() {
                open_licenses = true;
            }
        });
        if open_licenses {
            self.show_about = false;
            self.show_licenses = true;
        }
        if !open {
            self.show_about = false;
        }
    }

    fn draw_shortcuts_modal(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let open = centered_window(ctx, "Keyboard Shortcuts", true, Some([460.0, 520.0]), |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                shortcuts::draw_reference(ui);
            });
        });
        if !open {
            self.show_shortcuts = false;
        }
    }

    fn draw_control_window(&mut self, ctx: &egui::Context) {
        if !self.show_control {
            return;
        }
        let mut dirty = false;
        let mut apply = false;
        let status = control_status_label(&self.engine());
        let mut engine = self.engine();

        let open = centered_window(ctx, "OSC Control", false, Some([440.0, 420.0]), |ui| {
            let osc = &mut engine.state.cfg.control.osc;

            ui.heading("OSC input");
            if ui.checkbox(&mut osc.enabled, "Enabled").changed() {
                dirty = true;
            }

            egui::Grid::new("control_osc_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Bind address");
                    if ui.add(egui::TextEdit::singleline(&mut osc.bind).desired_width(160.0)).changed() {
                        dirty = true;
                    }
                    ui.end_row();

                    ui.label("Port");
                    if ui.add(egui::DragValue::new(&mut osc.port).range(1..=65535)).changed() {
                        dirty = true;
                    }
                    ui.end_row();

                    ui.label("Prefix");
                    if ui.add(
                            egui::TextEdit::singleline(&mut osc.prefix)
                                .hint_text("/multiviewer")
                                .desired_width(160.0),
                        )
                        .changed()
                    {
                        dirty = true;
                    }
                    ui.end_row();
                });
            ui.weak("Leave the prefix empty to answer at the root (/source/...).");

            ui.separator();
            ui.heading("Feedback output");
            if ui.radio_value(&mut osc.feedback_mode, FeedbackMode::Off, "No feedback").changed() {
                dirty = true;
            }
            if ui.radio_value(&mut osc.feedback_mode, FeedbackMode::Senders, "Reply to senders",).changed() {
                dirty = true;
            }
            if ui.radio_value(&mut osc.feedback_mode, FeedbackMode::Configured, "Configured target").changed() {
                dirty = true;
            }
            if ui.radio_value(&mut osc.feedback_mode, FeedbackMode::Both, "Senders + configured target").changed() {
                dirty = true;
            }

            if matches!(
                osc.feedback_mode,
                FeedbackMode::Configured | FeedbackMode::Both
            ) {
                egui::Grid::new("control_feedback_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Host");
                        if ui.add(
                                egui::TextEdit::singleline(&mut osc.feedback_host)
                                    .hint_text("127.0.0.1")
                                    .desired_width(160.0),
                            )
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.end_row();

                        ui.label("Port");
                        if ui.add(
                                egui::DragValue::new(&mut osc.feedback_port)
                                    .range(0..=65535),
                            )
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.end_row();
                    });
            }

            ui.add_space(8.0);
            ui.label(status.as_str());

            ui.separator();
            if ui.button("Apply").clicked() {
                apply = true;
            }
        });

        if dirty {
            engine.state.dirty = true;
        }
        if apply {
            engine.restart_control();
        }
        drop(engine);
        if !open {
            self.show_control = false;
        }
    }

    fn draw_licenses_modal(&mut self, ctx: &egui::Context) {
        if !self.show_licenses {
            return;
        }
        let open = centered_window(ctx, "Third-Party Licenses", true, Some([560.0, 620.0]), |ui| {
            ui.style_mut().url_in_tooltip = true;
            egui::ScrollArea::vertical().show(ui, |ui| {
                CommonMarkViewer::new().show(ui, &mut self.licenses_cache, LICENSES_TEXT);
            });
        });
        if !open {
            self.show_licenses = false;
        }
    }
}

/// A centred, non-movable window with the app's modal chrome. Returns whether
/// it should stay open (false once its title-bar X was used).
fn centered_window(
    ctx: &egui::Context,
    title: impl Into<egui::WidgetText>,
    resizable: bool,
    size: Option<[f32; 2]>,
    contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut open = true;
    let mut window = egui::Window::new(title)
        .collapsible(false)
        .resizable(resizable)
        .movable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(&mut open);
    if let Some(size) = size {
        window = window.default_size(size);
    }
    window.show(ctx, contents);
    return open;
}

/// Status line for the OSC Control window.
fn control_status_label(engine: &Engine) -> String {
    let listener = match engine.control.status() {
        ControlStatus::Stopped => "Stopped".to_string(),
        ControlStatus::Listening(addr) => format!("Listening on {addr}"),
        ControlStatus::Error(e) => format!("Error: {e}"),
    };
    let feedback = match engine.control.feedback_mode() {
        FeedbackMode::Off => "feedback off".to_string(),
        FeedbackMode::Senders => "feedback to senders".to_string(),
        FeedbackMode::Configured => match engine.control.feedback_target() {
            Some(addr) => format!("feedback to {addr}"),
            None => "feedback target unavailable".to_string(),
        },
        FeedbackMode::Both => match engine.control.feedback_target() {
            Some(addr) => format!("feedback to senders + {addr}"),
            None => "feedback to senders (target unavailable)".to_string(),
        },
    };
    return format!("{listener} · {feedback}");
}

/// Translate a keyboard shortcut into the shared control command vocabulary.
/// Returns `None` for actions that are UI-only (dialogs, confirmation).
fn shortcut_command(shortcut: Shortcut, engine: &Engine, nudge_amount: f32) -> Option<Command> {
    let selected = || engine.state.selected_source_id.clone().map(Selector::Key);
    return match shortcut {
        Shortcut::AddSource => Some(Command::AddSource),
        // Deleting while expanded would pull the view out from under the user.
        Shortcut::DeleteSource => {
            if engine.state.expanded_source_id.is_some() {
                None
            } else {
                selected().map(Command::RemoveSource)
            }
        }
        Shortcut::ExpandSource => {
            if engine.state.expanded_source_id.is_some() {
                Some(Command::ExitExpanded)
            } else {
                selected().map(Command::ExpandSource)
            }
        }
        Shortcut::ExitExpanded => {
            if engine.state.expanded_source_id.is_some() {
                Some(Command::ExitExpanded)
            } else {
                Some(Command::ClearSelection)
            }
        }
        Shortcut::SelectNextSource => Some(Command::SelectNext),
        Shortcut::SelectPreviousSource => Some(Command::SelectPrevious),
        Shortcut::NudgeUp => Some(Command::Nudge {
            source: None,
            dx: 0.0,
            dy: -nudge_amount,
        }),
        Shortcut::NudgeDown => Some(Command::Nudge {
            source: None,
            dx: 0.0,
            dy: nudge_amount,
        }),
        Shortcut::NudgeLeft => Some(Command::Nudge {
            source: None,
            dx: -nudge_amount,
            dy: 0.0,
        }),
        Shortcut::NudgeRight => Some(Command::Nudge {
            source: None,
            dx: nudge_amount,
            dy: 0.0,
        }),
        Shortcut::RecenterView => Some(Command::RecenterView),
        Shortcut::ZoomIn => Some(Command::Zoom { factor: 1.1 }),
        Shortcut::ZoomOut => Some(Command::Zoom { factor: 1.0 / 1.1 }),
        Shortcut::ToggleUi => Some(Command::ToggleUi),
        Shortcut::NewProject
        | Shortcut::OpenProject
        | Shortcut::SaveProject
        | Shortcut::SaveProjectAs => None,
    };
}

fn ensure_extension(mut path: std::path::PathBuf, ext: &str) -> std::path::PathBuf {
    if path.extension().and_then(|e| e.to_str()) != Some(ext) {
        path.set_extension(ext);
    }
    return path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Canvas;

    #[test]
    fn ensure_extension_appends_when_missing() {
        let path = PathBuf::from("/tmp/show");
        assert_eq!(
            ensure_extension(path, PROJECT_FILE_EXTENSION),
            PathBuf::from("/tmp/show.multiviewer")
        );
    }

    #[test]
    fn ensure_extension_keeps_when_present() {
        let path = PathBuf::from("/tmp/show.multiviewer");
        assert_eq!(
            ensure_extension(path, PROJECT_FILE_EXTENSION),
            PathBuf::from("/tmp/show.multiviewer")
        );
    }

    #[test]
    fn licenses_markdown_renders() {
        let mut cache = CommonMarkCache::default();
        egui::__run_test_ui(|ui| {
            CommonMarkViewer::new().show(ui, &mut cache, LICENSES_TEXT);
        });
    }

    #[test]
    fn shortcut_mapping_uses_the_control_commands() {
        let mut engine = Engine::test_instance(Canvas::default());
        let uuid = engine.add_source();
        engine.state.selected_source_id = Some(uuid.clone());

        assert_eq!(
            shortcut_command(Shortcut::AddSource, &engine, 1.0),
            Some(Command::AddSource)
        );
        assert_eq!(
            shortcut_command(Shortcut::DeleteSource, &engine, 1.0),
            Some(Command::RemoveSource(Selector::Key(uuid)))
        );
        assert_eq!(
            shortcut_command(Shortcut::NudgeUp, &engine, 1.0),
            Some(Command::Nudge {
                source: None,
                dx: 0.0,
                dy: -1.0
            })
        );
        assert_eq!(
            shortcut_command(Shortcut::NudgeRight, &engine, 10.0),
            Some(Command::Nudge {
                source: None,
                dx: 10.0,
                dy: 0.0
            })
        );
        assert_eq!(
            shortcut_command(Shortcut::ZoomIn, &engine, 1.0),
            Some(Command::Zoom { factor: 1.1 })
        );
        assert_eq!(
            shortcut_command(Shortcut::SelectNextSource, &engine, 1.0),
            Some(Command::SelectNext)
        );
        assert_eq!(shortcut_command(Shortcut::SaveProject, &engine, 1.0), None);
        assert_eq!(shortcut_command(Shortcut::NewProject, &engine, 1.0), None);
    }

    #[test]
    fn expanded_view_guards_shortcut_commands() {
        let mut engine = Engine::test_instance(Canvas::default());
        let uuid = engine.add_source();
        engine.state.selected_source_id = Some(uuid.clone());
        engine.state.expanded_source_id = Some(uuid);

        assert_eq!(shortcut_command(Shortcut::DeleteSource, &engine, 1.0), None);
        assert_eq!(
            shortcut_command(Shortcut::ExpandSource, &engine, 1.0),
            Some(Command::ExitExpanded)
        );
        assert_eq!(
            shortcut_command(Shortcut::ExitExpanded, &engine, 1.0),
            Some(Command::ExitExpanded)
        );
    }

    #[test]
    fn escape_clears_selection_when_not_expanded() {
        let mut engine = Engine::test_instance(Canvas::default());
        let uuid = engine.add_source();
        engine.state.selected_source_id = Some(uuid);

        assert_eq!(
            shortcut_command(Shortcut::ExitExpanded, &engine, 1.0),
            Some(Command::ClearSelection)
        );
    }
}
