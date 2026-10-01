use crate::engine::Engine;
use crate::ui::shortcuts::Shortcut;

/// A menu-bar item the user activated this frame.
pub enum MenuAction {
    Shortcut(Shortcut),
    ShowShortcuts,
    ShowAbout,
}

/// Draw the top menu bar (File / Edit / View / About).
pub fn draw(ui: &mut egui::Ui, engine: &Engine) -> Vec<MenuAction> {
    let mut actions = Vec::new();
    egui::Panel::top("menu_bar").show(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            menu_button(ui, "File", |ui| {
                menu_item(ui, &mut actions, Shortcut::NewProject, true);
                menu_item(ui, &mut actions, Shortcut::OpenProject, true);
                ui.separator();
                menu_item(ui, &mut actions, Shortcut::SaveProject, true);
                menu_item(ui, &mut actions, Shortcut::SaveProjectAs, true);
            });

            menu_button(ui, "Edit", |ui| {
                menu_item(ui, &mut actions, Shortcut::AddSource, true);
                menu_item(ui, &mut actions, Shortcut::DeleteSource, engine.selected_layer_id.is_some());
                ui.separator();
                menu_item(ui, &mut actions, Shortcut::ExpandSource, engine.selected_layer_id.is_some() || engine.expanded_layer_id.is_some());
                menu_item(ui, &mut actions, Shortcut::ExitExpanded, engine.expanded_layer_id.is_some());
                ui.separator();
                menu_item(ui, &mut actions, Shortcut::SelectPreviousSource, true);
                menu_item(ui, &mut actions, Shortcut::SelectNextSource, true);
                ui.separator();
                let nudge = engine.selected_layer_id.is_some();
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

            menu_button(ui, "About", |ui| {
                if ui.button(format!("About {}", crate::APP_NAME)).clicked() {
                    actions.push(MenuAction::ShowAbout);
                }
                if ui.button("Keyboard Shortcuts…").clicked() {
                    actions.push(MenuAction::ShowShortcuts);
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
