use std::path::PathBuf;

use crate::APP_NAME;
use crate::engine::Engine;
use crate::ui::shortcuts::Shortcut;

/// A menu-bar item the user activated this frame.
pub enum MenuAction {
    Shortcut(Shortcut),
    OpenRecent(PathBuf),
    SetAutoSaveInterval(u64),
    ShowShortcuts,
    ShowOSCControl,
    ShowAbout,
    ShowLicenses,
}

const AUTO_SAVE_INTERVALS: &[(u64, &str)] = &[
    (0, "Off"),
    (30, "30 seconds"),
    (60, "1 minute"),
    (300, "5 minutes"),
    (900, "15 minutes"),
    (1800, "30 minutes"),
    (3600, "1 hour"),
];

/// Draw the top menu bar (File / Edit / View / About).
pub fn draw(ui: &mut egui::Ui, engine: &Engine, recent: &[PathBuf]) -> Vec<MenuAction> {
    let mut actions = Vec::new();
    egui::Panel::top("menu_bar").show(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            menu_button(ui, "File", |ui| {
                menu_item(ui, &mut actions, Shortcut::NewProject, true);
                menu_item(ui, &mut actions, Shortcut::OpenProject, true);
                open_recent_menu(ui, &mut actions, recent);
                ui.separator();
                menu_item(ui, &mut actions, Shortcut::SaveProject, true);
                menu_item(ui, &mut actions, Shortcut::SaveProjectAs, true);
                ui.separator();
                ui.menu_button("Auto-save", |ui| {
                    let mut selected = engine.cfg.auto_save_interval;
                    for &(secs, label) in AUTO_SAVE_INTERVALS {
                        if ui.radio_value(&mut selected, secs, label).changed() {
                            actions.push(MenuAction::SetAutoSaveInterval(secs));
                            ui.close();
                        }
                    }
                });
            });

            menu_button(ui, "Edit", |ui| {
                menu_item(ui, &mut actions, Shortcut::AddSource, true);
                menu_item(ui, &mut actions, Shortcut::DeleteSource, engine.selected_source_id.is_some());
                ui.separator();
                menu_item(ui, &mut actions, Shortcut::ExpandSource, engine.selected_source_id.is_some() || engine.expanded_source_id.is_some());
                menu_item(ui, &mut actions, Shortcut::ExitExpanded, engine.expanded_source_id.is_some());
                ui.separator();
                menu_item(ui, &mut actions, Shortcut::SelectPreviousSource, true);
                menu_item(ui, &mut actions, Shortcut::SelectNextSource, true);
                ui.separator();
                let nudge = engine.selected_source_id.is_some();
                menu_item(ui, &mut actions, Shortcut::NudgeUp, nudge);
                menu_item(ui, &mut actions, Shortcut::NudgeDown, nudge);
                menu_item(ui, &mut actions, Shortcut::NudgeLeft, nudge);
                menu_item(ui, &mut actions, Shortcut::NudgeRight, nudge);
            });

            menu_button(ui, "View", |ui| {
                menu_item(ui, &mut actions, Shortcut::ToggleUi, true);
                ui.separator();
                menu_item(ui, &mut actions, Shortcut::ZoomIn, true);
                menu_item(ui, &mut actions, Shortcut::ZoomOut, true);
                menu_item(ui, &mut actions, Shortcut::RecenterView, true);
            });

            menu_button(ui, "Control", |ui| {
                if ui.button("Keyboard Shortcuts...").clicked() {
                    actions.push(MenuAction::ShowShortcuts);
                }
                if ui.button("OSC Settings...").clicked() {
                    actions.push(MenuAction::ShowOSCControl);
                }
            });

            menu_button(ui, "About", |ui| {
                if ui.button(format!("About {}", APP_NAME)).clicked() {
                    actions.push(MenuAction::ShowAbout);
                }
                if ui.button("Third-Party Licenses...").clicked() {
                    actions.push(MenuAction::ShowLicenses);
                }
            });
        });
    });
    return actions;
}

// Helpers
const MENU_BUTTON_PADDING: egui::Vec2 = egui::vec2(10.0, 0.0);

fn menu_button(ui: &mut egui::Ui, label: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.scope(|ui| {
        ui.style_mut().spacing.button_padding = MENU_BUTTON_PADDING;
        ui.menu_button(label, add_contents);
    });
}

fn menu_item(ui: &mut egui::Ui, actions: &mut Vec<MenuAction>, shortcut: Shortcut, enabled: bool) {
    let button = egui::Button::new(shortcut.label()).shortcut_text(shortcut.key());
    if ui.add_enabled(enabled, button).clicked() {
        actions.push(MenuAction::Shortcut(shortcut));
    }
}

fn open_recent_menu(ui: &mut egui::Ui, actions: &mut Vec<MenuAction>, recent: &[PathBuf]) {
    ui.menu_button("Open Recent", |ui| {
        if recent.is_empty() {
            ui.add_enabled(false, egui::Button::new("No Recent Projects"));
            return;
        }
        for path in recent {
            let label = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            if ui.button(label).on_hover_text(path.display().to_string()).clicked() {
                actions.push(MenuAction::OpenRecent(path.clone()));
                ui.close();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::AUTO_SAVE_INTERVALS;
    use crate::config::Config;

    #[test]
    fn default_auto_save_interval_is_selectable() {
        let default = Config::default().auto_save_interval;
        assert!(AUTO_SAVE_INTERVALS.iter().any(|&(secs, _)| secs == default));
    }
}
