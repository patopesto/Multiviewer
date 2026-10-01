use egui::{Key, Modifiers};

/// Active enumeration of all keyboard shortcuts in the UI.
///
/// This enum is the single source of truth for which shortcuts exist. The
/// `detect_*` functions centralise the actual key-to-action mapping so handlers
/// only need to match on the action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    // Global shortcuts — handled in App::ui.
    NewProject,
    OpenProject,
    SaveProject,
    SaveProjectAs,
    AddSource,
    DeleteSource,
    ExpandSource,
    ExitExpanded,
    NudgeUp,
    NudgeDown,
    NudgeLeft,
    NudgeRight,
    RecenterView,
    ZoomIn,
    ZoomOut,
    SelectNextSource,
    SelectPreviousSource,
    ToggleUi,
}

/// Platform modifier name used by [`Shortcut::key`].
#[cfg(target_os = "macos")]
const MOD: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
const MOD: &str = "Ctrl";

impl Shortcut {
    /// Human-readable action name (for help text / documentation).
    pub fn label(&self) -> &'static str {
        match self {
            Shortcut::NewProject => "New Project",
            Shortcut::OpenProject => "Open Project...",
            Shortcut::SaveProject => "Save Project",
            Shortcut::SaveProjectAs => "Save Project As...",
            Shortcut::AddSource => "Add Source",
            Shortcut::DeleteSource => "Delete Source",
            Shortcut::ExpandSource => "Expand Source",
            Shortcut::ExitExpanded => "Exit Expanded View",
            Shortcut::NudgeUp => "Nudge Source Up",
            Shortcut::NudgeDown => "Nudge Source Down",
            Shortcut::NudgeLeft => "Nudge Source Left",
            Shortcut::NudgeRight => "Nudge Source Right",
            Shortcut::RecenterView => "Re-Center Canvas",
            Shortcut::ZoomIn => "Zoom In",
            Shortcut::ZoomOut => "Zoom Out",
            Shortcut::SelectNextSource => "Select Next Source",
            Shortcut::SelectPreviousSource => "Select Previous Source",
            Shortcut::ToggleUi => "Hide UI",
        }
    }

    /// Human-readable key combo, platform-correct (for menus / documentation).
    pub fn key(&self) -> String {
        match self {
            Shortcut::NewProject => format!("{MOD} + N"),
            Shortcut::OpenProject => format!("{MOD} + O"),
            Shortcut::SaveProject => format!("{MOD} + S"),
            Shortcut::SaveProjectAs => format!("{MOD} + Shift + S"),
            Shortcut::AddSource => "A".into(),
            Shortcut::DeleteSource => "Delete".into(),
            Shortcut::ExpandSource => "F".into(),
            Shortcut::ExitExpanded => "Esc".into(),
            Shortcut::NudgeUp => "Shift + ↑".into(),
            Shortcut::NudgeDown => "Shift + ↓".into(),
            Shortcut::NudgeLeft => "Shift + ←".into(),
            Shortcut::NudgeRight => "Shift + →".into(),
            Shortcut::RecenterView => "0".into(),
            Shortcut::ZoomIn => "+".into(),
            Shortcut::ZoomOut => "-".into(),
            Shortcut::SelectNextSource => format!("{MOD} + Alt + ↓"),
            Shortcut::SelectPreviousSource => format!("{MOD} + Alt + ↑"),
            Shortcut::ToggleUi => "Space".into(),
        }
    }

    /// Modifier variants of a shortcut. Only shown in the shortcut reference —
    /// they would make menu items unreadably wide.
    fn detail(&self) -> &'static str {
        match self {
            Shortcut::NudgeUp => "  (Shift + Alt + ↑: 10 px)",
            Shortcut::NudgeDown => "  (Shift + Alt + ↓: 10 px)",
            Shortcut::NudgeLeft => "  (Shift + Alt + ←: 10 px)",
            Shortcut::NudgeRight => "  (Shift + Alt + →: 10 px)",
            _ => "",
        }
    }

    /// Menu this shortcut belongs to, used to group the shortcut reference.
    pub fn category(&self) -> &'static str {
        match self {
            Shortcut::NewProject
            | Shortcut::OpenProject
            | Shortcut::SaveProject
            | Shortcut::SaveProjectAs => "Project",
            Shortcut::AddSource
            | Shortcut::DeleteSource
            | Shortcut::ExpandSource
            | Shortcut::ExitExpanded
            | Shortcut::SelectNextSource
            | Shortcut::SelectPreviousSource => "Sources",
            Shortcut::NudgeUp
            | Shortcut::NudgeDown
            | Shortcut::NudgeLeft
            | Shortcut::NudgeRight
            | Shortcut::ToggleUi
            | Shortcut::ZoomIn
            | Shortcut::ZoomOut
            | Shortcut::RecenterView => "Canvas",
        }
    }

    /// Detect a global shortcut and consume the key event.
    ///
    /// Order matters: more specific modifier combinations are checked before
    /// simpler ones so they are not swallowed by their plain counterpart.
    pub fn detect_global(ctx: &egui::Context) -> Option<Self> {
        let shift_alt = Modifiers::SHIFT | Modifiers::ALT;
        let shift = Modifiers::SHIFT;

        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S)) {
            return Some(Shortcut::SaveProjectAs);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) {
            return Some(Shortcut::SaveProject);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::O)) {
            return Some(Shortcut::OpenProject);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::N)) {
            return Some(Shortcut::NewProject);
        }

        // Source selection (Ctrl/Cmd + Alt + arrow).
        let cmd_alt = Modifiers::COMMAND | Modifiers::ALT;
        if ctx.input_mut(|i| i.consume_key(cmd_alt, Key::ArrowDown)) {
            return Some(Shortcut::SelectNextSource);
        }
        if ctx.input_mut(|i| i.consume_key(cmd_alt, Key::ArrowUp)) {
            return Some(Shortcut::SelectPreviousSource);
        }

        // Fast nudge (Shift + Alt + arrow).
        if ctx.input_mut(|i| i.consume_key(shift_alt, Key::ArrowUp)) {
            return Some(Shortcut::NudgeUp);
        }
        if ctx.input_mut(|i| i.consume_key(shift_alt, Key::ArrowDown)) {
            return Some(Shortcut::NudgeDown);
        }
        if ctx.input_mut(|i| i.consume_key(shift_alt, Key::ArrowLeft)) {
            return Some(Shortcut::NudgeLeft);
        }
        if ctx.input_mut(|i| i.consume_key(shift_alt, Key::ArrowRight)) {
            return Some(Shortcut::NudgeRight);
        }

        // Nudge (Shift + arrow).
        if ctx.input_mut(|i| i.consume_key(shift, Key::ArrowUp)) {
            return Some(Shortcut::NudgeUp);
        }
        if ctx.input_mut(|i| i.consume_key(shift, Key::ArrowDown)) {
            return Some(Shortcut::NudgeDown);
        }
        if ctx.input_mut(|i| i.consume_key(shift, Key::ArrowLeft)) {
            return Some(Shortcut::NudgeLeft);
        }
        if ctx.input_mut(|i| i.consume_key(shift, Key::ArrowRight)) {
            return Some(Shortcut::NudgeRight);
        }

        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::A)) {
            return Some(Shortcut::AddSource);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete))
            || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Backspace))
        {
            return Some(Shortcut::DeleteSource);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F)) {
            return Some(Shortcut::ExpandSource);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            return Some(Shortcut::ExitExpanded);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Num0)) {
            return Some(Shortcut::RecenterView);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Plus))
            || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Equals))
        {
            return Some(Shortcut::ZoomIn);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Minus)) {
            return Some(Shortcut::ZoomOut);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Space)) {
            return Some(Shortcut::ToggleUi);
        }
        None
    }

    pub fn all_global() -> &'static [Shortcut] {
        &[
            Shortcut::NewProject,
            Shortcut::OpenProject,
            Shortcut::SaveProject,
            Shortcut::SaveProjectAs,
            Shortcut::AddSource,
            Shortcut::DeleteSource,
            Shortcut::ExpandSource,
            Shortcut::ExitExpanded,
            Shortcut::SelectPreviousSource,
            Shortcut::SelectNextSource,
            Shortcut::NudgeUp,
            Shortcut::NudgeDown,
            Shortcut::NudgeLeft,
            Shortcut::NudgeRight,
            Shortcut::RecenterView,
            Shortcut::ZoomIn,
            Shortcut::ZoomOut,
            Shortcut::ToggleUi,
        ]
    }
}

fn group_by_category(shortcuts: &[Shortcut]) -> Vec<(&'static str, Vec<Shortcut>)> {
    let mut groups: Vec<(&'static str, Vec<Shortcut>)> = Vec::new();
    for shortcut in shortcuts {
        let category = shortcut.category();
        match groups.iter_mut().find(|(c, _)| *c == category) {
            Some((_, items)) => items.push(*shortcut),
            None => groups.push((category, vec![*shortcut])),
        }
    }
    return groups;
}

/// Draw the shortcut reference as a two-column table grouped by category.
pub fn draw_reference(ui: &mut egui::Ui) {
    let groups = group_by_category(Shortcut::all_global());
    let row_height = ui.spacing().interact_size.y;

    egui_extras::TableBuilder::new(ui)
        .striped(true)
        .vscroll(false)
        .column(egui_extras::Column::remainder().at_least(240.0))
        .column(egui_extras::Column::auto().at_least(160.0))
        .body(|mut body| {
            for (category, shortcuts) in &groups {
                body.row(row_height + 8.0, |mut row| {
                    row.col(|ui| {
                        ui.label(egui::RichText::new(*category).heading().strong());
                    });
                    row.col(|_ui| {});
                });

                for shortcut in shortcuts {
                    body.row(row_height, |mut row| {
                        row.col(|ui| {
                            ui.label(shortcut.label());
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                ui.monospace(shortcut.key());
                                if !shortcut.detail().is_empty() {
                                    ui.weak(shortcut.detail().trim_start());
                                }
                            });
                        });
                    });
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_groups_each_category_once() {
        let groups = group_by_category(Shortcut::all_global());

        let mut distinct: Vec<&'static str> = Vec::new();
        for shortcut in Shortcut::all_global() {
            if !distinct.contains(&shortcut.category()) {
                distinct.push(shortcut.category());
            }
        }

        let grouped: Vec<&'static str> = groups.iter().map(|(c, _)| *c).collect();
        assert_eq!(grouped, distinct);
        assert_eq!(
            Shortcut::all_global().len(),
            groups.iter().map(|(_, items)| items.len()).sum::<usize>()
        );
    }

    // Tripwire: bump this when adding a shortcut, so the reference window
    // stays in sync with the keymap.
    #[test]
    fn all_global_lists_every_shortcut() {
        assert_eq!(Shortcut::all_global().len(), 18);
        for shortcut in Shortcut::all_global() {
            assert!(!shortcut.label().is_empty());
            assert!(!shortcut.key().is_empty());
        }
    }
}
