use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::super::{APP_NAME, PROJECT_FILE_EXTENSION};
use crate::compositor::Rect;
use crate::engine::Engine;
use crate::platform::poll_open_file;
use crate::session::Session;
use crate::ui::canvas;
use crate::ui::menu_bar::{self, MenuAction};
use crate::ui::side_panel;
use crate::ui::shortcuts::{self, Shortcut};

const DEFAULT_PROJECT_NAME: &str = "Untitled";

const UNFOCUSED_FPS: f64 = 60.0;
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
    ui_visible: bool,
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
        let mut app = Self {
            engine: Engine::new_project(),
            recent_projects: Vec::new(),
            last_error: None,
            last_warning: None,
            pending_confirm: None,
            show_about: false,
            show_shortcuts: false,
            ui_visible: true,
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
        self.render_stats.record_frame();

        self.handle_dropped_files(ctx);
        self.handle_platform_open_files();
        self.engine.update();
        self.engine.auto_save();
        self.update_title(ctx);

        if let Some(rs) = frame.wgpu_render_state() {
            self.engine.ensure_compositor(&rs.device, &rs.queue, rs.target_format);
            self.engine.begin_frame();
            self.engine.render_outputs();
        }

        if ctx.input(|i| i.viewport().focused).unwrap_or(true) {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_secs_f64(1.0 / UNFOCUSED_FPS));
        }
    }

    // Draw function
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if self.ui_visible {
            let actions = menu_bar::draw(ui, &self.engine, &self.recent_projects);
            side_panel::draw(ui, &mut self.engine);
            self.handle_menu_actions(&actions, ui);
        }
        self.handle_global_shortcuts(ui);
        self.draw_confirmation_modal(&ctx);
        self.draw_about_modal(&ctx);
        self.draw_shortcuts_modal(&ctx);
        if self.ui_visible {
            self.draw_status_bar(ui);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            canvas::update(ui, &mut self.engine, &ctx, frame, self.ui_visible);
        });
    }

    fn on_exit(&mut self) {
        drop(self.trace_guard.take());
    }
}


// Update stuff
impl App {
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
        if !self.ui_visible {
            match shortcut {
                Shortcut::ToggleUi | Shortcut::ExitExpanded => {}
                _ => return,
            }
        }

        self.apply_shortcut(shortcut, ui);
    }

    /// Run one shortcut. Shared by keyboard detection and the menu bar so both paths behave identically.
    fn apply_shortcut(&mut self, shortcut: Shortcut, ui: &egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let panel_rect = Rect {
            x: rect.min.x,
            y: rect.min.y,
            w: rect.width(),
            h: rect.height(),
        };
        let ctx = ui.ctx();
        let nudge_amount = if ctx.input(|i| i.modifiers.alt) { 10.0 } else { 1.0 };

        match shortcut {
            Shortcut::NewProject => self.confirm_or(Confirm::New),
            Shortcut::OpenProject => self.confirm_or(Confirm::Open),
            Shortcut::SaveProject => {
                if self.engine.project_path.is_some() {
                    if let Err(e) = self.engine.save_project() {
                        self.last_error = Some(e.to_string());
                    } else {
                        self.last_error = None;
                    }
                } else {
                    self.save_as_dialog();
                }
            }
            Shortcut::SaveProjectAs => self.save_as_dialog(),
            Shortcut::AddSource => {
                let uuid = self.engine.add_source();
                self.engine.selected_source_id = Some(uuid);
            }
            Shortcut::DeleteSource => {
                // Avoid deleting while a source is expanded; user can press Esc first.
                if self.engine.expanded_source_id.is_some() {
                    return;
                }
                if let Some(uuid) = self.engine.selected_source_id.take() {
                    self.engine.remove_source(&uuid);
                }
            }
            Shortcut::ExpandSource => {
                if self.engine.expanded_source_id.is_some() {
                    self.engine.clear_expanded_source();
                } else if let Some(uuid) = self.engine.selected_source_id.clone() {
                    self.engine.expand_source(uuid);
                }
            }
            Shortcut::ExitExpanded => {
                if self.engine.expanded_source_id.is_some() {
                    self.engine.clear_expanded_source();
                } else {
                    self.engine.selected_source_id = None;
                }
            }
            Shortcut::SelectNextSource => self.engine.select_source(1),
            Shortcut::SelectPreviousSource => self.engine.select_source(-1),
            Shortcut::NudgeUp => {
                self.engine.nudge_selected_source(0.0, -nudge_amount);
            }
            Shortcut::NudgeDown => {
                self.engine.nudge_selected_source(0.0, nudge_amount);
            }
            Shortcut::NudgeLeft => {
                self.engine.nudge_selected_source(-nudge_amount, 0.0);
            }
            Shortcut::NudgeRight => {
                self.engine.nudge_selected_source(nudge_amount, 0.0);
            }
            Shortcut::RecenterView => self.engine.recenter_view(&panel_rect),
            Shortcut::ZoomIn => self.engine.zoom_view(&panel_rect, 1.1),
            Shortcut::ZoomOut => self.engine.zoom_view(&panel_rect, 1.0 / 1.1),
            Shortcut::ToggleUi => self.ui_visible = !self.ui_visible,
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for file in dropped {
            if let Some(path) = file.path
                && path.extension().and_then(|e| e.to_str()) == Some(PROJECT_FILE_EXTENSION)
            {
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
        egui::Window::new(format!("About {}", APP_NAME))
            .collapsible(false)
            .resizable(false)
            .movable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.heading(APP_NAME);
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label(env!("CARGO_PKG_DESCRIPTION"));
            });
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
}
