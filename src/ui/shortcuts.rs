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
}

impl Shortcut {
    /// Human-readable action name (for help text / documentation).
    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            Shortcut::NewProject => "New project",
            Shortcut::OpenProject => "Open project",
            Shortcut::SaveProject => "Save project",
            Shortcut::SaveProjectAs => "Save project as",
            Shortcut::AddSource => "Add source",
            Shortcut::DeleteSource => "Delete selected source",
            Shortcut::ExpandSource => "Expand selected source",
            Shortcut::ExitExpanded => "Exit expanded view",
            Shortcut::NudgeUp => "Nudge source up",
            Shortcut::NudgeDown => "Nudge source down",
            Shortcut::NudgeLeft => "Nudge source left",
            Shortcut::NudgeRight => "Nudge source right",
            Shortcut::RecenterView => "Re-centre view",
            Shortcut::ZoomIn => "Zoom in",
            Shortcut::ZoomOut => "Zoom out",
            Shortcut::SelectNextSource => "Select next source",
            Shortcut::SelectPreviousSource => "Select previous source",
        }
    }

    /// Human-readable key combo (for help text / documentation).
    #[allow(dead_code)]
    pub fn key(&self) -> &'static str {
        match self {
            Shortcut::NewProject => "Ctrl/Cmd + N",
            Shortcut::OpenProject => "Ctrl/Cmd + O",
            Shortcut::SaveProject => "Ctrl/Cmd + S",
            Shortcut::SaveProjectAs => "Ctrl/Cmd + Shift + S",
            Shortcut::AddSource => "A",
            Shortcut::DeleteSource => "Delete / Backspace",
            Shortcut::ExpandSource => "F",
            Shortcut::ExitExpanded => "Esc",
            Shortcut::NudgeUp => "Shift + ↑  (Shift + Alt + ↑ for 10 px)",
            Shortcut::NudgeDown => "Shift + ↓  (Shift + Alt + ↓ for 10 px)",
            Shortcut::NudgeLeft => "Shift + ←  (Shift + Alt + ← for 10 px)",
            Shortcut::NudgeRight => "Shift + →  (Shift + Alt + → for 10 px)",
            Shortcut::RecenterView => "0",
            Shortcut::ZoomIn => "+ / =",
            Shortcut::ZoomOut => "-",
            Shortcut::SelectNextSource => "Ctrl/Cmd + Alt + ↓",
            Shortcut::SelectPreviousSource => "Ctrl/Cmd + Alt + ↑",
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
        None
    }

    #[allow(dead_code)]
    pub fn all_global() -> &'static [Shortcut] {
        &[
            Shortcut::SaveProjectAs,
            Shortcut::SaveProject,
            Shortcut::OpenProject,
            Shortcut::NewProject,
            Shortcut::NudgeUp,
            Shortcut::NudgeDown,
            Shortcut::NudgeLeft,
            Shortcut::NudgeRight,
            Shortcut::AddSource,
            Shortcut::DeleteSource,
            Shortcut::ExpandSource,
            Shortcut::ExitExpanded,
            Shortcut::RecenterView,
            Shortcut::ZoomIn,
            Shortcut::ZoomOut,
            Shortcut::SelectNextSource,
            Shortcut::SelectPreviousSource,
        ]
    }
}

/// Generate a plain-text reference of all shortcuts for help windows or docs.
#[allow(dead_code)]
pub fn reference_text() -> String {
    let mut out = String::new();
    out.push_str("Global shortcuts:\n");
    for s in Shortcut::all_global() {
        out.push_str(&format!("  {:45} {}\n", s.key(), s.label()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_text_lists_all_shortcuts() {
        let text = reference_text();
        assert!(text.contains("Add source"));
        assert!(text.contains("F"));
        assert!(text.contains("Esc"));
        assert!(text.contains("Nudge source up"));
        assert!(text.contains("Select next source"));
    }
}
