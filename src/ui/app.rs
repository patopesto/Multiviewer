use std::path::PathBuf;

use super::super::{APP_NAME, PROJECT_FILE_EXTENSION};
use crate::engine::Engine;
use crate::ui::side_panel::FileAction;

const DEFAULT_PROJECT_NAME: &str = "Untitled";

pub struct App {
    engine: Engine,
    image_loaders_installed: bool,
    last_error: Option<String>,
    pending_confirm: Option<Confirm>,
}

enum Confirm {
    New,
    Open,
    OpenPath(PathBuf),
}

impl App {
    pub fn new(startup_path: Option<PathBuf>) -> Self {
        let mut app = Self {
            engine: Engine::new_project(),
            image_loaders_installed: false,
            last_error: None,
            pending_confirm: None,
        };
        if let Some(path) = startup_path {
            if path.exists() {
                if let Err(e) = app.engine.open_project(&path) {
                    app.last_error = Some(e.to_string());
                }
            } else {
                app.last_error = Some(format!("Startup project not found: {}", path.display()));
            }
        }
        app
    }

    fn open_path(&mut self, path: &std::path::Path) {
        if let Err(e) = self.engine.open_project(path) {
            self.last_error = Some(e.to_string());
        } else {
            self.last_error = None;
        }
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
            }
        }
    }

    fn handle_actions(&mut self, actions: Vec<FileAction>) {
        for action in actions {
            match action {
                FileAction::New => self.confirm_or(Confirm::New),
                FileAction::Open => self.confirm_or(Confirm::Open),
                FileAction::Save => {
                    if let Err(e) = self.engine.save_project() {
                        self.last_error = Some(e.to_string());
                    } else {
                        self.last_error = None;
                    }
                }
                FileAction::SaveAs => self.save_as_dialog(),
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for file in dropped {
            if let Some(path) = file.path {
                if path.extension().and_then(|e| e.to_str()) == Some(PROJECT_FILE_EXTENSION) {
                    self.confirm_or(Confirm::Open);
                    break;
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn handle_macos_open_files(&mut self) {
        if let Some(path) = crate::macos_app::drain_queue() {
            if path.extension().and_then(|e| e.to_str()) == Some(PROJECT_FILE_EXTENSION) {
                self.confirm_or(Confirm::OpenPath(path));
            }
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
        let mut open = true;
        let mut action: Option<Confirm> = None;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .movable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("You have unsaved changes. Discard them?");
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        self.pending_confirm = None;
                    }
                    if ui.button("Discard").clicked() {
                        action = self.pending_confirm.take();
                    }
                });
            });
        if !open {
            self.pending_confirm = None;
        }
        if let Some(action) = action {
            self.execute_confirm(action);
        }
    }

    fn draw_status_bar(&self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            let status = if let Some(err) = &self.last_error {
                format!("Error: {err}")
            } else if self.engine.dirty {
                if self.engine.project_path.is_some() {
                    "Unsaved changes".to_string()
                } else {
                    "Untitled — use Save As".to_string()
                }
            } else {
                "Saved".to_string()
            };
            ui.label(status);
        });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.image_loaders_installed {
            egui_extras::install_image_loaders(ctx);
            self.image_loaders_installed = true;
        }
        self.handle_dropped_files(ctx);
        #[cfg(target_os = "macos")]
        self.handle_macos_open_files();
        self.engine.update();
        self.engine.auto_save();
        self.update_title(ctx);

        if let Some(rs) = frame.wgpu_render_state() {
            self.engine.ensure_compositor(&rs.device, &rs.queue, rs.target_format);
            self.engine.render_outputs();
        }

        // Keep the UI rendering continuously; without this egui only repaints on input events.
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let actions = super::side_panel::draw(ui, &mut self.engine);
        self.handle_actions(actions);
        self.draw_confirmation_modal(&ctx);
        self.draw_status_bar(ui);

        egui::CentralPanel::default().show(ui, |ui| {
            super::canvas::update(ui, &mut self.engine, &ctx, frame);
        });
    }
}

fn ensure_extension(mut path: std::path::PathBuf, ext: &str) -> std::path::PathBuf {
    if path.extension().and_then(|e| e.to_str()) != Some(ext) {
        path.set_extension(ext);
    }
    path
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
