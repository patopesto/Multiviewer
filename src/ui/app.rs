use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::super::{APP_NAME, PROJECT_FILE_EXTENSION};
use crate::control::{Command, Selector};
use crate::engine::Engine;
use crate::platform::poll_open_file;
use crate::session::Session;
use crate::ui::canvas;
use crate::ui::menu_bar::{self, MenuAction};
use crate::ui::side_panel;
use crate::ui::shortcuts::{self, Shortcut};

const DEFAULT_PROJECT_NAME: &str = "Untitled";
const LICENSES_TEXT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/LICENSE.md"));

const ACTIVE_FPS: f64 = 60.0;
const IDLE_FPS: f64 = 10.0;
const FRAME_TOLERANCE: Duration = Duration::from_millis(2);
const RENDER_FPS_WINDOW: Duration = Duration::from_secs(2);
const MAX_RENDER_TIMESTAMPS: usize = 256;

pub struct App {
    engine: Engine,
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
    next_frame: Instant,
    next_output_frame: Instant,
    render_stats: RenderStats,
    trace_guard: Option<tracing_chrome::FlushGuard>,
}

// Moving-window FPS tracker over logic() calls.
struct RenderStats {
    recent_frames: VecDeque<Instant>,
}

impl RenderStats {
    fn new() -> Self {
        Self {
            recent_frames: VecDeque::new(),
        }
    }

    fn record_frame(&mut self) {
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

    fn fps(&self) -> f64 {
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
    ) -> Self {
        let (gpu_backend, gpu_name) = match &cc.wgpu_render_state {
            Some(render_state) => {
                let info = render_state.adapter.get_info();
                (Some(format!("{:?}", info.backend)), Some(info.name))
            }
            None => (None, None),
        };
        let mut app = Self {
            engine: Engine::new_project(),
            recent_projects: Vec::new(),
            last_error: None,
            last_warning: None,
            pending_confirm: None,
            show_about: false,
            show_shortcuts: false,
            show_control: false,
            show_licenses: false,
            licenses_cache: CommonMarkCache::default(),
            vendor_versions: Engine::vendor_versions(),
            gpu_backend,
            gpu_name,
            next_frame: Instant::now(),
            next_output_frame: Instant::now(),
            render_stats: RenderStats::new(),
            trace_guard,
        };

        Self::configure_egui(&cc.egui_ctx);

        if let Some(path) = startup_path {
            if path.exists() {
                if let Err(e) = app.engine.open_project(&path) {
                    app.last_error = Some(e.to_string());
                } else {
                    app.refresh_warning();
                }
            } else {
                app.last_error = Some(format!("Startup project not found: {}", path.display()));
            }
        }
        app.refresh_recent();
        return app;
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
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.handle_dropped_files(ctx);
        self.handle_platform_open_files();
        self.update_title(ctx);

        // App focused: run at display refresh rate.
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        if focused {
            self.run_frame(frame);
            ctx.request_repaint();
            return;
        }

        // App not focused: cap whether any output is enabled
        let target_fps = if self.engine.output_registry.any_enabled()
            || self.engine.control.is_enabled()
        {
            ACTIVE_FPS
        } else {
            IDLE_FPS
        };
        let interval = Duration::from_secs_f64(1.0 / target_fps);

        let now = Instant::now();
        if now + FRAME_TOLERANCE < self.next_frame {
            ctx.request_repaint_after(self.next_frame - now);
            return;
        }
        self.next_frame = advance_deadline(self.next_frame, now, interval);

        self.run_frame(frame);
        ctx.request_repaint();
    }

    // Draw function
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if self.engine.ui_visible {
            let actions = menu_bar::draw(ui, &self.engine, &self.recent_projects);
            side_panel::draw(ui, &mut self.engine);
            self.handle_menu_actions(&actions, ui);
        }
        self.handle_global_shortcuts(ui);
        self.draw_confirmation_modal(&ctx);
        self.draw_about_modal(&ctx);
        self.draw_shortcuts_modal(&ctx);
        self.draw_control_window(&ctx);
        self.draw_licenses_modal(&ctx);
        if self.engine.ui_visible {
            self.draw_status_bar(ui);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            canvas::update(ui, &mut self.engine, &ctx, frame);
        });
    }

    fn on_exit(&mut self) {
        drop(self.trace_guard.take());
    }
}

// Update stuff
impl App {
    /// One frame of engine work: discovery, auto-save, compositing and outputs.
    fn run_frame(&mut self, frame: &mut eframe::Frame) {
        self.render_stats.record_frame();
        self.engine.update();
        self.engine.auto_save();

        let Some(rs) = frame.wgpu_render_state() else {
            return;
        };
        self.engine.ensure_compositor(&rs.device, &rs.queue, rs.target_format);
        self.engine.begin_frame();

        // Outputs run at their own fixed rate
        let now = Instant::now();
        if now + FRAME_TOLERANCE >= self.next_output_frame {
            let interval = Duration::from_secs_f64(1.0 / ACTIVE_FPS);
            self.next_output_frame = advance_deadline(self.next_output_frame, now, interval);
            self.engine.render_outputs();
        }
    }

    /// Promote warnings collected while loading the current project to the status bar.
    fn refresh_warning(&mut self) {
        self.last_warning = if self.engine.load_warnings.is_empty() {
            None
        } else {
            Some(self.engine.load_warnings.join(" "))
        };
    }

    fn open_path(&mut self, path: &std::path::Path) {
        if let Err(e) = self.engine.open_project(path) {
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
        if self.engine.dirty {
            self.pending_confirm = Some(action);
        } else {
            self.execute_confirm(action);
        }
    }

    fn execute_confirm(&mut self, action: Confirm) {
        match action {
            Confirm::New => {
                self.engine = Engine::new_project();
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
            if let Err(e) = self.engine.save_project_as(&path) {
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
        if !self.engine.ui_visible {
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
                if self.engine.project_path.is_some() {
                    if let Err(e) = self.engine.apply_command(Command::SaveProject) {
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
        let Some(command) = shortcut_command(shortcut, &self.engine, nudge_amount) else {
            return;
        };
        if let Err(e) = self.engine.apply_command(command) {
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
        let filename = self
            .engine
            .project_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| DEFAULT_PROJECT_NAME.to_string());
        let prefix = if self.engine.dirty { "* " } else { "" };
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
        if self.engine.dirty {
            if self.engine.project_path.is_some() {
                return "Unsaved changes".to_string();
            }
            return "Untitled — use Save As".to_string();
        }
        return "Saved".to_string();
    }

    fn draw_status_bar(&self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.status_text());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{:.0} FPS", self.render_stats.fps()));
                });
            });
        });
    }

    fn draw_about_modal(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut open = true;
        let mut open_licenses = false;
        egui::Window::new(format!("About {}", APP_NAME))
            .collapsible(false)
            .resizable(false)
            .movable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
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
        let mut open = true;
        egui::Window::new("Keyboard Shortcuts")
            .collapsible(false)
            .resizable(true)
            .movable(false)
            .default_size([460.0, 520.0])
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
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
        let mut open = true;
        let mut dirty = false;
        let mut apply = false;
        let status = self.engine.control_status_label();

        egui::Window::new("OSC Control")
            .collapsible(false)
            .resizable(false)
            .movable(false)
            .default_size([420.0, 220.0])
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                let osc = &mut self.engine.cfg.control.osc;

                if ui.checkbox(&mut osc.enabled, "Enabled").changed() {
                    dirty = true;
                }

                egui::Grid::new("control_osc_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Bind address");
                        if ui
                            .add(egui::TextEdit::singleline(&mut osc.bind).desired_width(160.0))
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.end_row();

                        ui.label("Port");
                        if ui
                            .add(egui::DragValue::new(&mut osc.port).range(1..=65535))
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.end_row();

                        ui.label("Prefix");
                        if ui
                            .add(
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

                ui.add_space(8.0);
                ui.label(status.as_str());

                ui.separator();
                if ui.button("Apply").clicked() {
                    apply = true;
                }
            });

        if dirty {
            self.engine.dirty = true;
        }
        if apply {
            self.engine.restart_control();
        }
        if !open {
            self.show_control = false;
        }
    }

    fn draw_licenses_modal(&mut self, ctx: &egui::Context) {
        if !self.show_licenses {
            return;
        }
        let mut open = true;
        egui::Window::new("Third-Party Licenses")
            .collapsible(false)
            .resizable(true)
            .movable(false)
            .default_size([560.0, 620.0])
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
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

/// Translate a keyboard shortcut into the shared control command vocabulary.
/// Returns `None` for actions that are UI-only (dialogs, confirmation).
fn shortcut_command(shortcut: Shortcut, engine: &Engine, nudge_amount: f32) -> Option<Command> {
    let selected = || engine.selected_source_id.clone().map(Selector::Key);
    return match shortcut {
        Shortcut::AddSource => Some(Command::AddSource),
        // Deleting while expanded would pull the view out from under the user.
        Shortcut::DeleteSource => {
            if engine.expanded_source_id.is_some() {
                None
            } else {
                selected().map(Command::RemoveSource)
            }
        }
        Shortcut::ExpandSource => {
            if engine.expanded_source_id.is_some() {
                Some(Command::ExitExpanded)
            } else {
                selected().map(Command::ExpandSource)
            }
        }
        Shortcut::ExitExpanded => {
            if engine.expanded_source_id.is_some() {
                Some(Command::ExitExpanded)
            } else {
                Some(Command::ClearSelection)
            }
        }
        Shortcut::SelectNextSource => Some(Command::SelectNext),
        Shortcut::SelectPreviousSource => Some(Command::SelectPrevious),
        Shortcut::NudgeUp => Some(Command::Nudge {
            dx: 0.0,
            dy: -nudge_amount,
        }),
        Shortcut::NudgeDown => Some(Command::Nudge {
            dx: 0.0,
            dy: nudge_amount,
        }),
        Shortcut::NudgeLeft => Some(Command::Nudge {
            dx: -nudge_amount,
            dy: 0.0,
        }),
        Shortcut::NudgeRight => Some(Command::Nudge {
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

fn advance_deadline(prev: Instant, now: Instant, interval: Duration) -> Instant {
    return if now >= prev + interval {
        now + interval
    } else {
        prev + interval
    };
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
    fn deadline_advances_on_a_fixed_grid_when_running_early() {
        let interval = Duration::from_millis(16);
        let deadline = Instant::now() + interval;
        // Ran 2ms before the deadline: the next deadline must still be a full
        // interval after the current one, not 2ms sooner.
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

    #[test]
    fn shortcut_mapping_uses_the_control_commands() {
        let mut engine = Engine::test_instance(Canvas::default());
        let uuid = engine.add_source();
        engine.selected_source_id = Some(uuid.clone());

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
                dx: 0.0,
                dy: -1.0
            })
        );
        assert_eq!(
            shortcut_command(Shortcut::NudgeRight, &engine, 10.0),
            Some(Command::Nudge {
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
        engine.selected_source_id = Some(uuid.clone());
        engine.expanded_source_id = Some(uuid);

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
        engine.selected_source_id = Some(uuid);

        assert_eq!(
            shortcut_command(Shortcut::ExitExpanded, &engine, 1.0),
            Some(Command::ClearSelection)
        );
    }
}
