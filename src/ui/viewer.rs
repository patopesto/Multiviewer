use std::path::PathBuf;
use std::sync::mpsc::Sender;

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::super::{APP_NAME, PROJECT_FILE_EXTENSION};
use crate::compositor::Rect;
use crate::config::Config;
use crate::control::{FeedbackMode, HttpControlConfig, OscControlConfig};
use crate::engine::{Command, Engine, EngineSnapshot, FocusHandle, PreviewFrame, Selector};
use crate::session::Session;
use crate::sources::Protocol;
use crate::ui::canvas::{self, CanvasUi};
use crate::ui::menu_bar::{self, MenuAction};
use crate::ui::side_panel::{self, SidePanel};
use crate::ui::shortcuts::{self, Shortcut};

const DEFAULT_PROJECT_NAME: &str = "Untitled";
const LICENSES_TEXT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/LICENSE.md"));

/// Which host is showing this viewer. Only the file dialogs differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiMode {
    Native,
    Remote,
}

/// A project action only the host can run (dialogs, project files).
#[derive(Clone, Debug, PartialEq)]
pub enum HostAction {
    NewProject,
    OpenDialog,
    OpenPath(PathBuf),
    SaveAsDialog,
    SaveAsPath(PathBuf),
}

#[derive(Clone, Debug)]
enum Confirm {
    New,
    OpenPath(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathPrompt {
    Open,
    SaveAs,
}

/// Per-viewer inputs the host builds each frame.
pub struct HostView<'a> {
    pub preview: Option<&'a PreviewFrame>,
    pub focus: Option<&'a FocusHandle>,
    pub status: &'a str,
    pub status_tooltip: &'a str,
    pub web_status: Option<&'a str>,
}

pub struct DrawOutput {
    pub actions: Vec<HostAction>,
    pub canvas_rect: Rect,
}

pub struct Viewer {
    pub mode: UiMode,
    pub canvas_ui: CanvasUi,
    pub panel: SidePanel,
    pub recent_projects: Vec<PathBuf>,
    pub last_error: Option<String>,
    pub last_warning: Option<String>,
    pub vendor_versions: Vec<(&'static str, Option<String>)>,
    pub gpu_backend: Option<String>,
    pub gpu_name: Option<String>,
    pending_confirm: Option<Confirm>,
    show_about: bool,
    show_shortcuts: bool,
    show_osc: bool,
    show_http: bool,
    show_licenses: bool,
    licenses_cache: CommonMarkCache,
    osc_draft: OscControlConfig,
    http_draft: HttpControlConfig,
    path_prompt: Option<PathPrompt>,
    path_input: String,
}

impl Viewer {
    pub fn new(mode: UiMode) -> Self {
        return Self {
            mode,
            canvas_ui: CanvasUi::default(),
            panel: SidePanel::default(),
            recent_projects: Session::load().recent_projects,
            last_error: None,
            last_warning: None,
            vendor_versions: Engine::vendor_versions(),
            gpu_backend: None,
            gpu_name: None,
            pending_confirm: None,
            show_about: false,
            show_shortcuts: false,
            show_osc: false,
            show_http: false,
            show_licenses: false,
            licenses_cache: CommonMarkCache::default(),
            osc_draft: OscControlConfig::default(),
            http_draft: HttpControlConfig::default(),
            path_prompt: None,
            path_input: String::new(),
        };
    }

    /// One full UI pass. The host executes the returned actions.
    pub fn draw(&mut self, ui: &mut egui::Ui, snapshot: &EngineSnapshot, commands: &Sender<Command>, host: HostView<'_>) -> DrawOutput {
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();
        let ui_visible = snapshot.state.ui_visible;

        if let Some(focus) = host.focus {
            focus.set(selected_protocol(snapshot).as_ref());
        }

        let alt = ui.ctx().input(|input| input.modifiers.alt);
        if ui_visible {
            let menu_actions = menu_bar::draw(ui, snapshot, &self.recent_projects);
            side_panel::draw(ui, snapshot, &mut self.panel, commands);
            self.handle_menu_actions(menu_actions, snapshot, commands, alt, &mut actions);
        }
        self.handle_global_shortcuts(ui, snapshot, commands, alt, &mut actions);
        self.draw_confirmation_modal(&ctx, &mut actions);
        self.draw_path_prompt(&ctx, &mut actions);
        self.draw_about_modal(&ctx);
        self.draw_shortcuts_modal(&ctx);
        self.draw_osc_window(&ctx, snapshot, commands);
        self.draw_http_window(&ctx, snapshot, commands, host.web_status);
        self.draw_licenses_modal(&ctx);
        if ui_visible {
            let left = self.status_text(snapshot);
            self.draw_status_bar(ui, &left, host.status, host.status_tooltip);
        }

        let canvas_rect = egui::CentralPanel::default()
            .show(ui, |ui| canvas::update(ui, snapshot, commands, &mut self.canvas_ui, host.preview, &ctx))
            .inner;

        return DrawOutput { actions, canvas_rect };
    }

    /// Refresh after the host replaced the document.
    pub fn after_project_loaded(&mut self, engine: &Engine) {
        self.panel = SidePanel::default();
        self.canvas_ui = CanvasUi::default();
        self.pending_confirm = None;
        self.last_warning = if engine.load_warnings.is_empty() {
            None
        } else {
            Some(engine.load_warnings.join(" "))
        };
        self.recent_projects = Session::load().recent_projects;
    }

    pub fn open_osc_settings(&mut self, snapshot: &EngineSnapshot) {
        self.osc_draft = snapshot.state.cfg.control.osc.clone();
        self.show_osc = true;
        self.show_http = false;
    }

    pub fn open_http_settings(&mut self, snapshot: &EngineSnapshot) {
        self.http_draft = snapshot.state.cfg.control.http.clone();
        self.show_http = true;
        self.show_osc = false;
    }

    fn handle_menu_actions(
        &mut self,
        menu_actions: Vec<MenuAction>,
        snapshot: &EngineSnapshot,
        commands: &Sender<Command>,
        alt: bool,
        actions: &mut Vec<HostAction>,
    ) {
        if self.pending_confirm.is_some() {
            return;
        }
        for action in menu_actions {
            match action {
                MenuAction::Shortcut(shortcut) => {
                    self.apply_shortcut(shortcut, snapshot, commands, alt, actions);
                }
                MenuAction::OpenRecent(path) => self.request_open(path, snapshot, actions),
                MenuAction::SetAutoSaveInterval(seconds) => {
                    let _ = commands.send(Command::SetAutoSaveInterval(seconds));
                }
                MenuAction::ShowAbout => self.show_about = true,
                MenuAction::ShowShortcuts => self.show_shortcuts = true,
                MenuAction::ShowOscSettings => self.open_osc_settings(snapshot),
                MenuAction::ShowHttpSettings => self.open_http_settings(snapshot),
                MenuAction::ShowLicenses => self.show_licenses = true,
            }
        }
    }

    fn handle_global_shortcuts(
        &mut self,
        ui: &egui::Ui,
        snapshot: &EngineSnapshot,
        commands: &Sender<Command>,
        alt: bool,
        actions: &mut Vec<HostAction>,
    ) {
        let ctx = ui.ctx();
        // Don't fire shortcuts while a modal is open or while typing in a text field.
        if self.pending_confirm.is_some() || ctx.text_edit_focused() {
            return;
        }
        let Some(shortcut) = Shortcut::detect_global(ctx) else {
            return;
        };
        if !snapshot.state.ui_visible {
            match shortcut {
                Shortcut::ToggleUi | Shortcut::ExitExpanded => {}
                _ => return,
            }
        }
        self.apply_shortcut(shortcut, snapshot, commands, alt, actions);
    }

    fn apply_shortcut(
        &mut self,
        shortcut: Shortcut,
        snapshot: &EngineSnapshot,
        commands: &Sender<Command>,
        alt: bool,
        actions: &mut Vec<HostAction>,
    ) {
        match shortcut {
            Shortcut::NewProject => {
                self.request_new(snapshot, actions);
                return;
            }
            Shortcut::OpenProject => {
                self.request_open_dialog(snapshot, actions);
                return;
            }
            Shortcut::SaveProjectAs => {
                self.request_save_as_dialog(snapshot, actions);
                return;
            }
            Shortcut::SaveProject => {
                if snapshot.state.project_path.is_some() {
                    let _ = commands.send(Command::SaveProject);
                } else {
                    self.request_save_as_dialog(snapshot, actions);
                }
                return;
            }
            _ => {}
        }

        let nudge_amount = if alt { 10.0 } else { 1.0 };
        let Some(command) = shortcut_command(shortcut, snapshot, nudge_amount) else {
            return;
        };
        let _ = commands.send(command);
    }

    fn request_new(&mut self, snapshot: &EngineSnapshot, actions: &mut Vec<HostAction>) {
        if snapshot.state.dirty {
            self.pending_confirm = Some(Confirm::New);
        } else {
            actions.push(HostAction::NewProject);
        }
    }

    fn request_open_dialog(&mut self, snapshot: &EngineSnapshot, actions: &mut Vec<HostAction>) {
        if self.mode == UiMode::Native {
            actions.push(HostAction::OpenDialog);
            return;
        }
        self.path_input = snapshot.state.project_path.as_ref().map(|path| path.display().to_string()).unwrap_or_default();
        self.path_prompt = Some(PathPrompt::Open);
    }

    pub fn request_open(&mut self, path: PathBuf, snapshot: &EngineSnapshot, actions: &mut Vec<HostAction>) {
        if snapshot.state.dirty {
            self.pending_confirm = Some(Confirm::OpenPath(path));
        } else {
            actions.push(HostAction::OpenPath(path));
        }
    }

    fn request_save_as_dialog(&mut self, snapshot: &EngineSnapshot, actions: &mut Vec<HostAction>) {
        if self.mode == UiMode::Native {
            actions.push(HostAction::SaveAsDialog);
            return;
        }
        self.path_input = snapshot
            .state
            .project_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| format!("{DEFAULT_PROJECT_NAME}.{PROJECT_FILE_EXTENSION}"));
        self.path_prompt = Some(PathPrompt::SaveAs);
    }

    fn draw_confirmation_modal(&mut self, ctx: &egui::Context, actions: &mut Vec<HostAction>) {
        let Some(pending) = self.pending_confirm.clone() else {
            return;
        };
        let title = match pending {
            Confirm::New => "New project",
            Confirm::OpenPath(_) => "Open project",
        };
        let mut action = None;
        let response = egui::Modal::new(egui::Id::new("confirm_modal")).backdrop_color(egui::Color32::from_black_alpha(175)).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.label(egui::RichText::new(title).heading().strong());
            ui.add_space(12.0);
            ui.label("You have unsaved changes. Discard them?");
            ui.add_space(20.0);
            ui.separator();
            egui::Sides::new().show(
                ui,
                |_ui| {},
                |ui| {
                    if ui.add(egui::Button::new("Discard").fill(egui::Color32::from_rgb(140, 30, 30))).clicked() {
                        action = self.pending_confirm.take();
                    }
                    if ui.button("Cancel").clicked() {
                        self.pending_confirm = None;
                    }
                },
            );
        });
        if response.should_close() {
            self.pending_confirm = None;
        }
        if let Some(action) = action {
            match action {
                Confirm::New => actions.push(HostAction::NewProject),
                Confirm::OpenPath(path) => actions.push(HostAction::OpenPath(path)),
            }
        }
    }

    fn draw_path_prompt(&mut self, ctx: &egui::Context, actions: &mut Vec<HostAction>) {
        let Some(prompt) = self.path_prompt else {
            return;
        };
        let mut submit = false;
        let mut cancel = false;
        let title = match prompt {
            PathPrompt::Open => "Open project",
            PathPrompt::SaveAs => "Save project as",
        };
        let open = centered_window(ctx, title, false, Some([520.0, 160.0]), |ui| {
            ui.label("Project path on the Multiviewer machine:");
            let response = ui.add(egui::TextEdit::singleline(&mut self.path_input).desired_width(f32::INFINITY).hint_text("/shows/main.multiviewer"));
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            ui.add_space(8.0);
            egui::Sides::new().show(
                ui,
                |_ui| {},
                |ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let label = match prompt {
                        PathPrompt::Open => "Open",
                        PathPrompt::SaveAs => "Save",
                    };
                    if ui.add_enabled(!self.path_input.trim().is_empty(), egui::Button::new(label)).clicked() {
                        submit = true;
                    }
                },
            );
        });
        if cancel || !open {
            self.path_prompt = None;
            return;
        }
        if submit {
            let path = PathBuf::from(self.path_input.trim());
            self.path_prompt = None;
            match prompt {
                PathPrompt::Open => actions.push(HostAction::OpenPath(path)),
                PathPrompt::SaveAs => actions.push(HostAction::SaveAsPath(ensure_extension(path, PROJECT_FILE_EXTENSION))),
            }
        }
    }

    fn draw_osc_window(&mut self, ctx: &egui::Context, snapshot: &EngineSnapshot, commands: &Sender<Command>) {
        if !self.show_osc {
            return;
        }
        let mut apply = false;
        let status = snapshot.runtime.control.osc.as_str();
        let open = centered_window(ctx, "OSC Settings", false, Some([500.0, 560.0]), |ui| {
            let osc = &mut self.osc_draft;
            ui.checkbox(&mut osc.enabled, "Enabled");
            egui::Grid::new("control_osc_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Bind address");
                ui.add(egui::TextEdit::singleline(&mut osc.bind).desired_width(160.0));
                ui.end_row();
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut osc.port).range(1..=65535));
                ui.end_row();
                ui.label("Prefix");
                ui.add(egui::TextEdit::singleline(&mut osc.prefix).hint_text("/multiviewer").desired_width(160.0));
                ui.end_row();
            });
            ui.weak("Leave the prefix empty to answer at the root (/source/...).");
            ui.label(status);
            ui.separator();

            ui.heading("Feedback output");
            let mode = &mut osc.feedback_mode;
            ui.radio_value(mode, FeedbackMode::Off, "No feedback");
            ui.radio_value(mode, FeedbackMode::Senders, "Reply to senders");
            ui.radio_value(mode, FeedbackMode::Configured, "Configured target");
            ui.radio_value(mode, FeedbackMode::Both, "Senders + configured target");
            if matches!(osc.feedback_mode, FeedbackMode::Configured | FeedbackMode::Both) {
                egui::Grid::new("control_feedback_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Host");
                    ui.add(egui::TextEdit::singleline(&mut osc.feedback_host).hint_text("127.0.0.1").desired_width(160.0));
                    ui.end_row();
                    ui.label("Port");
                    ui.add(egui::DragValue::new(&mut osc.feedback_port).range(0..=65535));
                    ui.end_row();
                });
            }

            ui.separator();
            apply = ui.add_enabled(self.osc_draft != snapshot.state.cfg.control.osc, egui::Button::new("Apply")).clicked();
        });
        if apply {
            let mut control = snapshot.state.cfg.control.clone();
            control.osc = self.osc_draft.clone();
            let _ = commands.send(Command::PatchControl { control });
        }
        if !open {
            self.show_osc = false;
        }
    }

    fn draw_http_window(&mut self, ctx: &egui::Context, snapshot: &EngineSnapshot, commands: &Sender<Command>, web_status: Option<&str>) {
        if !self.show_http {
            return;
        }
        let mut apply = false;
        let web = web_status.unwrap_or("HTTP stopped");
        let link_url = {
            let running = &snapshot.state.cfg.control.http;
            if running.enabled && !running.bind.trim().is_empty() {
                Some(match self.mode {
                    UiMode::Remote => "/".to_string(),
                    UiMode::Native => http_ui_url(&running.bind, running.port),
                })
            } else {
                None
            }
        };
        let open = centered_window(ctx, "HTTP Settings", false, Some([500.0, 320.0]), |ui| {
            let http = &mut self.http_draft;
            ui.checkbox(&mut http.enabled, "Enabled");
            egui::Grid::new("control_http_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Bind address");
                ui.add(egui::TextEdit::singleline(&mut http.bind).desired_width(160.0));
                ui.end_row();
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut http.port).range(1..=65535));
                ui.end_row();
            });
            ui.label(web);
            if let Some(url) = &link_url {
                ui.hyperlink_to("Open the web UI", url);
            }
            ui.weak("The API is not authenticated. Bind to a trusted network only.");

            ui.separator();
            apply = ui.add_enabled(self.http_draft != snapshot.state.cfg.control.http, egui::Button::new("Apply")).clicked();
        });
        if apply {
            let mut control = snapshot.state.cfg.control.clone();
            control.http = self.http_draft.clone();
            let _ = commands.send(Command::PatchControl { control });
        }
        if !open {
            self.show_http = false;
        }
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

    fn status_text(&self, snapshot: &EngineSnapshot) -> String {
        if let Some(err) = &self.last_error {
            return format!("Error: {err}");
        }
        if let Some(warn) = &self.last_warning {
            return format!("Warning: {warn}");
        }
        if snapshot.state.dirty {
            if snapshot.state.project_path.is_some() {
                return "Unsaved changes".to_string();
            }
            return "Untitled — use Save As".to_string();
        }
        return "Saved".to_string();
    }

    fn draw_status_bar(&self, ui: &mut egui::Ui, left: &str, status: &str, tooltip: &str) {
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(left);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(status).on_hover_text(tooltip);
                });
            });
        });
    }
}

impl Viewer {
    /// Run a host action on the engine. Shared by the native window and the
    /// remote session so both load projects with the same rules.
    pub fn run_action(&mut self, engine: &mut Engine, action: &HostAction) -> Result<(), String> {
        match action {
            HostAction::NewProject => engine.new_in_place(self.mode == UiMode::Remote).map_err(|e| e.to_string())?,
            HostAction::OpenPath(path) => {
                if self.mode == UiMode::Remote {
                    let cfg = Config::load_from(path).map_err(|e| e.to_string())?;
                    engine.replace_project(cfg, Some(path.clone()), true).map_err(|e| e.to_string())?;
                } else {
                    engine.open_project(path).map_err(|e| e.to_string())?;
                }
            }
            HostAction::SaveAsPath(path) => engine.save_project_as(path).map_err(|e| e.to_string())?,
            HostAction::OpenDialog | HostAction::SaveAsDialog => {
                return Err("dialog action cannot run in this host".to_string());
            }
        }
        self.last_error = None;
        self.recent_projects = Session::load().recent_projects;
        if matches!(action, HostAction::NewProject | HostAction::OpenPath(_)) {
            self.after_project_loaded(engine);
        }
        return Ok(());
    }
}

/// The protocol of the selected placement, used for the discovery focus.
pub fn selected_protocol(snapshot: &EngineSnapshot) -> Option<Protocol> {
    return snapshot
        .state
        .selected_source_id
        .as_ref()
        .and_then(|id| snapshot.state.cfg.canvas.sources.iter().find(|source| &source.uuid == id).map(|source| source.protocol.clone()));
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

/// Translate a keyboard shortcut into the shared control command vocabulary.
/// Returns `None` for actions that are UI-only (dialogs, confirmation).
fn shortcut_command(shortcut: Shortcut, snapshot: &EngineSnapshot, nudge_amount: f32) -> Option<Command> {
    let selected = || snapshot.state.selected_source_id.clone().map(Selector::Id);
    return match shortcut {
        Shortcut::AddSource => Some(Command::AddSource),
        // A duplicate created while expanded is invisible and would take the
        // selection off the expanded source.
        Shortcut::DuplicateSource => {
            if snapshot.state.expanded_source_id.is_some() {
                None
            } else {
                selected().map(Command::DuplicateSource)
            }
        }
        // Deleting while expanded would pull the view out from under the user.
        Shortcut::DeleteSource => {
            if snapshot.state.expanded_source_id.is_some() {
                None
            } else {
                selected().map(Command::RemoveSource)
            }
        }
        Shortcut::ExpandSource => {
            if snapshot.state.expanded_source_id.is_some() {
                Some(Command::ExitExpanded)
            } else {
                selected().map(Command::ExpandSource)
            }
        }
        Shortcut::ExitExpanded => {
            if snapshot.state.expanded_source_id.is_some() {
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
        Shortcut::ToggleStats => Some(Command::ToggleStats),
        Shortcut::NewProject | Shortcut::OpenProject | Shortcut::SaveProject | Shortcut::SaveProjectAs => None,
    };
}

fn http_ui_url(bind: &str, port: u16) -> String {
    let bind = bind.trim();
    let host = match bind {
        "0.0.0.0" => "127.0.0.1".to_string(),
        "::" => "[::1]".to_string(),
        value if value.starts_with('[') => value.to_string(),
        value if value.contains(':') => format!("[{value}]"),
        value => value.to_string(),
    };
    return format!("http://{host}:{port}/");
}

pub(crate) fn ensure_extension(mut path: PathBuf, ext: &str) -> PathBuf {
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
        assert_eq!(ensure_extension(path, PROJECT_FILE_EXTENSION), PathBuf::from("/tmp/show.multiviewer"));
    }

    #[test]
    fn ensure_extension_keeps_when_present() {
        let path = PathBuf::from("/tmp/show.multiviewer");
        assert_eq!(ensure_extension(path, PROJECT_FILE_EXTENSION), PathBuf::from("/tmp/show.multiviewer"));
    }

    #[test]
    fn licenses_markdown_renders() {
        let mut cache = CommonMarkCache::default();
        egui::__run_test_ui(|ui| {
            CommonMarkViewer::new().show(ui, &mut cache, LICENSES_TEXT);
        });
    }

    #[test]
    fn http_ui_url_handles_unspecified_and_ipv6_binds() {
        assert_eq!(http_ui_url("0.0.0.0", 8090), "http://127.0.0.1:8090/");
        assert_eq!(http_ui_url("::", 8090), "http://[::1]:8090/");
        assert_eq!(http_ui_url("::1", 8090), "http://[::1]:8090/");
        assert_eq!(http_ui_url("[::1]", 8090), "http://[::1]:8090/");
        assert_eq!(http_ui_url("192.168.1.5", 8090), "http://192.168.1.5:8090/");
        assert_eq!(http_ui_url("multiviewer.lan", 8090), "http://multiviewer.lan:8090/");
    }

    #[test]
    fn opening_a_settings_window_leaves_the_other_draft_alone() {
        let mut engine = Engine::test_instance(Canvas::default());
        engine.state.cfg.control.osc.port = 9100;
        engine.state.cfg.control.http.port = 8200;
        let snapshot = engine.snapshot();

        let mut viewer = Viewer::new(UiMode::Native);
        viewer.open_osc_settings(&snapshot);
        assert_eq!(viewer.osc_draft.port, 9100);
        assert_eq!(viewer.http_draft.port, 8090);

        viewer.http_draft.port = 9999;
        viewer.open_osc_settings(&snapshot);
        assert_eq!(viewer.osc_draft.port, 9100);
        assert_eq!(viewer.http_draft.port, 9999);

        viewer.open_http_settings(&snapshot);
        assert_eq!(viewer.http_draft.port, 8200);
    }

    #[test]
    fn shortcut_mapping_uses_the_control_commands() {
        let mut engine = Engine::test_instance(Canvas::default());
        let uuid = engine.add_source();
        engine.state.selected_source_id = Some(uuid.clone());
        let snapshot = engine.snapshot();

        assert_eq!(shortcut_command(Shortcut::AddSource, &snapshot, 1.0), Some(Command::AddSource));
        assert_eq!(
            shortcut_command(Shortcut::DuplicateSource, &snapshot, 1.0),
            Some(Command::DuplicateSource(Selector::Id(uuid.clone())))
        );
        assert_eq!(
            shortcut_command(Shortcut::DeleteSource, &snapshot, 1.0),
            Some(Command::RemoveSource(Selector::Id(uuid)))
        );
        assert_eq!(
            shortcut_command(Shortcut::NudgeUp, &snapshot, 1.0),
            Some(Command::Nudge {
                source: None,
                dx: 0.0,
                dy: -1.0
            })
        );
        assert_eq!(
            shortcut_command(Shortcut::NudgeRight, &snapshot, 10.0),
            Some(Command::Nudge {
                source: None,
                dx: 10.0,
                dy: 0.0
            })
        );
        assert_eq!(shortcut_command(Shortcut::ZoomIn, &snapshot, 1.0), Some(Command::Zoom { factor: 1.1 }));
        assert_eq!(shortcut_command(Shortcut::SelectNextSource, &snapshot, 1.0), Some(Command::SelectNext));
        assert_eq!(shortcut_command(Shortcut::SaveProject, &snapshot, 1.0), None);
        assert_eq!(shortcut_command(Shortcut::NewProject, &snapshot, 1.0), None);
    }

    #[test]
    fn expanded_view_guards_shortcut_commands() {
        let mut engine = Engine::test_instance(Canvas::default());
        let uuid = engine.add_source();
        engine.state.selected_source_id = Some(uuid.clone());
        engine.state.expanded_source_id = Some(uuid);
        let snapshot = engine.snapshot();

        assert_eq!(shortcut_command(Shortcut::DuplicateSource, &snapshot, 1.0), None);
        assert_eq!(shortcut_command(Shortcut::DeleteSource, &snapshot, 1.0), None);
        assert_eq!(shortcut_command(Shortcut::ExpandSource, &snapshot, 1.0), Some(Command::ExitExpanded));
        assert_eq!(shortcut_command(Shortcut::ExitExpanded, &snapshot, 1.0), Some(Command::ExitExpanded));
    }

    #[test]
    fn escape_clears_selection_when_not_expanded() {
        let mut engine = Engine::test_instance(Canvas::default());
        let uuid = engine.add_source();
        engine.state.selected_source_id = Some(uuid);
        let snapshot = engine.snapshot();

        assert_eq!(shortcut_command(Shortcut::ExitExpanded, &snapshot, 1.0), Some(Command::ClearSelection));
    }
}
