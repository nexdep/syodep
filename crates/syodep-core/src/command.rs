//! The command set.
//!
//! Every user-visible action is a [`Command`]. Keybindings map key sequences
//! to command *names*; the UI never calls behavior directly. This indirection
//! is what will later let us add a command palette, text objects and
//! user-defined bindings without touching event handlers.
//!
//! Every variant here must be documented in `docs/commands.md`.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    // Scrolling.
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
    ScrollHalfPageDown,
    ScrollHalfPageUp,
    ScrollPageDown,
    ScrollPageUp,
    // Page navigation.
    NextPage,
    PrevPage,
    /// With a count, jumps to that page; otherwise the first page.
    GotoFirstPage,
    /// With a count, jumps to that page; otherwise the last page.
    GotoLastPage,
    // Zoom.
    ZoomIn,
    ZoomOut,
    FitWidth,
    ZoomReset,
    /// Scroll so the current highlight is centered in the viewport (Vim `zz`).
    CenterView,
    // Focus mode (one highlighted position at the active scope).
    //
    // Entering is also how the scope is changed: `cw` from normal mode enters
    // focus word-granular, and `cw` while already focused re-reads the current
    // position as a word without moving it.
    /// Enter focus mode keeping the current scope — char from normal mode,
    /// which resets it, and the live scope from visual mode.
    FocusEnter,
    /// Focus the nearest content character by character.
    FocusEnterChar,
    /// Focus the nearest content word by word.
    FocusEnterWord,
    /// Focus the nearest content line by line.
    FocusEnterLine,
    /// Focus the nearest content sentence by sentence.
    FocusEnterSentence,
    /// Focus the nearest content paragraph by paragraph.
    FocusEnterParagraph,
    /// Leave focus mode, returning to scrolling (the position is remembered).
    FocusExit,
    /// Move one unit of the active scope leftwards.
    FocusLeft,
    /// Move one unit of the active scope rightwards.
    FocusRight,
    /// Move up (line-wise for char/word scope, else the previous unit).
    FocusUp,
    /// Move down (line-wise for char/word scope, else the next unit).
    FocusDown,
    /// Move to the start of the next word, whatever the scope.
    FocusNextWord,
    /// Move to the start of the previous word, whatever the scope.
    FocusPrevWord,
    /// Move to the start of the next line, whatever the scope.
    FocusNextLine,
    /// Move to the start of the next sentence, whatever the scope.
    FocusNextSentence,
    /// Move to the start of the next paragraph, whatever the scope.
    FocusNextParagraph,
    // Visual mode (a two-ended selection over content).
    /// Enter visual mode, inheriting the current focus mode's granularity.
    VisualEnter,
    /// Enter visual mode selecting character by character.
    VisualEnterChar,
    /// Enter visual mode selecting word by word.
    VisualEnterWord,
    /// Enter visual mode selecting line by line.
    VisualEnterLine,
    /// Enter visual mode selecting sentence by sentence.
    VisualEnterSentence,
    /// Enter visual mode selecting paragraph by paragraph.
    VisualEnterParagraph,
    /// Leave visual mode, returning to the mode it was entered from.
    VisualExit,
    /// Grow or shrink the selection leftwards by one unit of the active scope.
    VisualLeft,
    /// Grow or shrink the selection rightwards by one unit of the active scope.
    VisualRight,
    /// Move the active end up (line-wise for char/word scope, else previous unit).
    VisualUp,
    /// Move the active end down (line-wise for char/word scope, else next unit).
    VisualDown,
    /// Move the active end to the start of the next word, whatever the scope.
    VisualNextWord,
    /// Move the active end to the start of the previous word, whatever the scope.
    VisualPrevWord,
    /// Move the active end to the start of the next line, whatever the scope.
    VisualNextLine,
    /// Move the active end to the start of the next sentence, whatever the scope.
    VisualNextSentence,
    /// Move the active end to the start of the next paragraph, whatever the scope.
    VisualNextParagraph,
    /// Make the other end of the selection the active one.
    VisualSwapEnds,
    /// Set the active end's granularity to characters.
    VisualScopeChar,
    /// Set the active end's granularity to words.
    VisualScopeWord,
    /// Set the active end's granularity to lines.
    VisualScopeLine,
    /// Set the active end's granularity to sentences.
    VisualScopeSentence,
    /// Set the active end's granularity to paragraphs.
    VisualScopeParagraph,
    /// Switch to the other end and set its granularity to characters.
    VisualOtherChar,
    /// Switch to the other end and set its granularity to words.
    VisualOtherWord,
    /// Switch to the other end and set its granularity to lines.
    VisualOtherLine,
    /// Switch to the other end and set its granularity to sentences.
    VisualOtherSentence,
    /// Switch to the other end and set its granularity to paragraphs.
    VisualOtherParagraph,
    // Highlight mode (a selection on its way to becoming a highlight).
    //
    // There is no `highlight_left` and there never will be: reshaping a pending
    // highlight *is* reshaping a selection, so highlight mode binds the
    // `visual_*` motions rather than duplicating them.
    /// Turn the focus highlight or selection into a pending highlight.
    HighlightEnter,
    /// Store the pending highlight and return to focus mode on the moving end.
    HighlightCommit,
    /// Throw the pending highlight away, restoring the mode and selection that
    /// were in effect when it was started.
    HighlightDiscard,
    // Keybinding-help overlay. These navigation commands are only reachable
    // through the overlay's isolated keymap.
    /// Show or hide the modal keybinding-help overlay.
    ToggleKeybindingsOverlay,
    /// Scroll the keybinding-help overlay down by one row.
    HelpScrollDown,
    /// Scroll the keybinding-help overlay up by one row.
    HelpScrollUp,
    /// Scroll the keybinding-help overlay down by half a viewport.
    HelpHalfPageDown,
    /// Scroll the keybinding-help overlay up by half a viewport.
    HelpHalfPageUp,
    /// Scroll the keybinding-help overlay down by one viewport.
    HelpPageDown,
    /// Scroll the keybinding-help overlay up by one viewport.
    HelpPageUp,
    /// Scroll the keybinding-help overlay to its first row.
    HelpTop,
    /// Scroll the keybinding-help overlay to its last row.
    HelpBottom,
    // Application.
    /// Show or hide the highlights sidebar, moving keyboard focus with it.
    ToggleHighlightsSidebar,
    /// Show or hide the annotations sidebar, moving keyboard focus with it.
    ToggleAnnotationsSidebar,
    /// Capture the current focus or selection as a pending Markdown annotation
    /// and ask the shell to open the annotation editor.
    CreateAnnotation,
    OpenFile,
    /// Overwrite the open PDF with the highlights embedded in it.
    SaveDocument,
    Quit,
    /// Clears pending input. Reserved to also dismiss UI state later.
    Cancel,
}

/// All commands with their canonical names, for documentation and
/// "unknown command" error messages.
pub const ALL_COMMANDS: &[(&str, Command)] = &[
    ("scroll_down", Command::ScrollDown),
    ("scroll_up", Command::ScrollUp),
    ("scroll_left", Command::ScrollLeft),
    ("scroll_right", Command::ScrollRight),
    ("scroll_half_page_down", Command::ScrollHalfPageDown),
    ("scroll_half_page_up", Command::ScrollHalfPageUp),
    ("scroll_page_down", Command::ScrollPageDown),
    ("scroll_page_up", Command::ScrollPageUp),
    ("next_page", Command::NextPage),
    ("prev_page", Command::PrevPage),
    ("goto_first_page", Command::GotoFirstPage),
    ("goto_last_page", Command::GotoLastPage),
    ("zoom_in", Command::ZoomIn),
    ("zoom_out", Command::ZoomOut),
    ("fit_width", Command::FitWidth),
    ("zoom_reset", Command::ZoomReset),
    ("center_view", Command::CenterView),
    ("focus_enter", Command::FocusEnter),
    ("focus_enter_char", Command::FocusEnterChar),
    ("focus_enter_word", Command::FocusEnterWord),
    ("focus_enter_line", Command::FocusEnterLine),
    ("focus_enter_sentence", Command::FocusEnterSentence),
    ("focus_enter_paragraph", Command::FocusEnterParagraph),
    ("focus_exit", Command::FocusExit),
    ("focus_left", Command::FocusLeft),
    ("focus_right", Command::FocusRight),
    ("focus_up", Command::FocusUp),
    ("focus_down", Command::FocusDown),
    ("focus_next_word", Command::FocusNextWord),
    ("focus_prev_word", Command::FocusPrevWord),
    ("focus_next_line", Command::FocusNextLine),
    ("focus_next_sentence", Command::FocusNextSentence),
    ("focus_next_paragraph", Command::FocusNextParagraph),
    ("visual_enter", Command::VisualEnter),
    ("visual_enter_char", Command::VisualEnterChar),
    ("visual_enter_word", Command::VisualEnterWord),
    ("visual_enter_line", Command::VisualEnterLine),
    ("visual_enter_sentence", Command::VisualEnterSentence),
    ("visual_enter_paragraph", Command::VisualEnterParagraph),
    ("visual_exit", Command::VisualExit),
    ("visual_left", Command::VisualLeft),
    ("visual_right", Command::VisualRight),
    ("visual_up", Command::VisualUp),
    ("visual_down", Command::VisualDown),
    ("visual_next_word", Command::VisualNextWord),
    ("visual_prev_word", Command::VisualPrevWord),
    ("visual_next_line", Command::VisualNextLine),
    ("visual_next_sentence", Command::VisualNextSentence),
    ("visual_next_paragraph", Command::VisualNextParagraph),
    ("visual_swap_ends", Command::VisualSwapEnds),
    ("visual_scope_char", Command::VisualScopeChar),
    ("visual_scope_word", Command::VisualScopeWord),
    ("visual_scope_line", Command::VisualScopeLine),
    ("visual_scope_sentence", Command::VisualScopeSentence),
    ("visual_scope_paragraph", Command::VisualScopeParagraph),
    ("visual_other_char", Command::VisualOtherChar),
    ("visual_other_word", Command::VisualOtherWord),
    ("visual_other_line", Command::VisualOtherLine),
    ("visual_other_sentence", Command::VisualOtherSentence),
    ("visual_other_paragraph", Command::VisualOtherParagraph),
    ("highlight_enter", Command::HighlightEnter),
    ("highlight_commit", Command::HighlightCommit),
    ("highlight_discard", Command::HighlightDiscard),
    (
        "toggle_keybindings_overlay",
        Command::ToggleKeybindingsOverlay,
    ),
    ("help_scroll_down", Command::HelpScrollDown),
    ("help_scroll_up", Command::HelpScrollUp),
    ("help_half_page_down", Command::HelpHalfPageDown),
    ("help_half_page_up", Command::HelpHalfPageUp),
    ("help_page_down", Command::HelpPageDown),
    ("help_page_up", Command::HelpPageUp),
    ("help_top", Command::HelpTop),
    ("help_bottom", Command::HelpBottom),
    (
        "toggle_highlights_sidebar",
        Command::ToggleHighlightsSidebar,
    ),
    (
        "toggle_annotations_sidebar",
        Command::ToggleAnnotationsSidebar,
    ),
    ("create_annotation", Command::CreateAnnotation),
    ("open_file", Command::OpenFile),
    ("save_document", Command::SaveDocument),
    ("quit", Command::Quit),
    ("cancel", Command::Cancel),
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown command {name:?}")]
pub struct UnknownCommand {
    pub name: String,
}

impl FromStr for Command {
    type Err = UnknownCommand;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ALL_COMMANDS
            .iter()
            .find(|(name, _)| *name == s)
            .map(|(_, command)| *command)
            .ok_or_else(|| UnknownCommand { name: s.to_owned() })
    }
}

impl Command {
    pub fn name(self) -> &'static str {
        ALL_COMMANDS
            .iter()
            .find(|(_, command)| *command == self)
            .map(|(name, _)| *name)
            .expect("every command has an entry in ALL_COMMANDS")
    }

    /// Concise user-facing label for command discovery surfaces.
    pub fn description(self) -> &'static str {
        match self {
            Self::ScrollDown => "Scroll down",
            Self::ScrollUp => "Scroll up",
            Self::ScrollLeft => "Scroll left",
            Self::ScrollRight => "Scroll right",
            Self::ScrollHalfPageDown => "Scroll half a page down",
            Self::ScrollHalfPageUp => "Scroll half a page up",
            Self::ScrollPageDown => "Scroll one page down",
            Self::ScrollPageUp => "Scroll one page up",
            Self::NextPage => "Go to the next page",
            Self::PrevPage => "Go to the previous page",
            Self::GotoFirstPage => "Go to the first page",
            Self::GotoLastPage => "Go to the last page",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
            Self::FitWidth => "Fit page width",
            Self::ZoomReset => "Reset zoom",
            Self::CenterView => "Center the current focus",
            Self::FocusEnter => "Enter focus mode",
            Self::FocusEnterChar => "Focus by character",
            Self::FocusEnterWord => "Focus by word",
            Self::FocusEnterLine => "Focus by line",
            Self::FocusEnterSentence => "Focus by sentence",
            Self::FocusEnterParagraph => "Focus by paragraph",
            Self::FocusExit => "Leave focus mode",
            Self::FocusLeft => "Move focus left",
            Self::FocusRight => "Move focus right",
            Self::FocusUp => "Move focus up",
            Self::FocusDown => "Move focus down",
            Self::FocusNextWord => "Move focus to the next word",
            Self::FocusPrevWord => "Move focus to the previous word",
            Self::FocusNextLine => "Move focus to the next line",
            Self::FocusNextSentence => "Move focus to the next sentence",
            Self::FocusNextParagraph => "Move focus to the next paragraph",
            Self::VisualEnter => "Enter visual mode",
            Self::VisualEnterChar => "Select by character",
            Self::VisualEnterWord => "Select by word",
            Self::VisualEnterLine => "Select by line",
            Self::VisualEnterSentence => "Select by sentence",
            Self::VisualEnterParagraph => "Select by paragraph",
            Self::VisualExit => "Leave visual mode",
            Self::VisualLeft => "Move the active selection end left",
            Self::VisualRight => "Move the active selection end right",
            Self::VisualUp => "Move the active selection end up",
            Self::VisualDown => "Move the active selection end down",
            Self::VisualNextWord => "Move the active end to the next word",
            Self::VisualPrevWord => "Move the active end to the previous word",
            Self::VisualNextLine => "Move the active end to the next line",
            Self::VisualNextSentence => "Move the active end to the next sentence",
            Self::VisualNextParagraph => "Move the active end to the next paragraph",
            Self::VisualSwapEnds => "Switch the active selection end",
            Self::VisualScopeChar => "Set the active end to character scope",
            Self::VisualScopeWord => "Set the active end to word scope",
            Self::VisualScopeLine => "Set the active end to line scope",
            Self::VisualScopeSentence => "Set the active end to sentence scope",
            Self::VisualScopeParagraph => "Set the active end to paragraph scope",
            Self::VisualOtherChar => "Switch ends with character scope",
            Self::VisualOtherWord => "Switch ends with word scope",
            Self::VisualOtherLine => "Switch ends with line scope",
            Self::VisualOtherSentence => "Switch ends with sentence scope",
            Self::VisualOtherParagraph => "Switch ends with paragraph scope",
            Self::HighlightEnter => "Start a highlight",
            Self::HighlightCommit => "Keep the pending highlight",
            Self::HighlightDiscard => "Discard the pending highlight",
            Self::ToggleKeybindingsOverlay => "Show or hide keybinding help",
            Self::HelpScrollDown => "Scroll help down",
            Self::HelpScrollUp => "Scroll help up",
            Self::HelpHalfPageDown => "Scroll help half a page down",
            Self::HelpHalfPageUp => "Scroll help half a page up",
            Self::HelpPageDown => "Scroll help one page down",
            Self::HelpPageUp => "Scroll help one page up",
            Self::HelpTop => "Go to the start of help",
            Self::HelpBottom => "Go to the end of help",
            Self::ToggleHighlightsSidebar => "Toggle the Highlights sidebar",
            Self::ToggleAnnotationsSidebar => "Toggle the Annotations sidebar",
            Self::CreateAnnotation => "Create a Markdown annotation",
            Self::OpenFile => "Open a PDF",
            Self::SaveDocument => "Save highlights into the PDF",
            Self::Quit => "Quit syodep",
            Self::Cancel => "Cancel the current action",
        }
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_commands() {
        assert_eq!(
            "scroll_down".parse::<Command>().unwrap(),
            Command::ScrollDown
        );
        assert_eq!("quit".parse::<Command>().unwrap(), Command::Quit);
    }

    #[test]
    fn unknown_command_reports_name() {
        let err = "warp_speed".parse::<Command>().unwrap_err();
        assert_eq!(err.name, "warp_speed");
    }

    #[test]
    fn names_round_trip() {
        for (name, command) in ALL_COMMANDS {
            assert_eq!(command.name(), *name);
            assert_eq!(name.parse::<Command>().unwrap(), *command);
            assert!(!command.description().is_empty());
        }
    }
}
