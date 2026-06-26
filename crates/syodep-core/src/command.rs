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
    // Caret (modal cursor over text + images).
    /// Enter caret focus mode, placing the caret on the nearest content.
    CaretFocusEnter,
    /// Leave caret focus mode, returning to scrolling (the caret is remembered).
    CaretFocusExit,
    CaretFocusLeft,
    CaretFocusRight,
    CaretFocusUp,
    CaretFocusDown,
    CaretFocusNextWord,
    CaretFocusEndWord,
    CaretFocusPrevWord,
    // Line focus (modal whole-line highlight over content).
    /// Enter line focus mode, highlighting the nearest content line.
    LineFocusEnter,
    /// Leave line focus mode, returning to scrolling (the line is remembered).
    LineFocusExit,
    /// Move to the line in the previous column (multi-column pages only).
    LineFocusLeft,
    /// Move to the line in the next column (multi-column pages only).
    LineFocusRight,
    LineFocusUp,
    LineFocusDown,
    // Word focus (modal whole-word highlight over content).
    /// Enter word focus mode, highlighting the nearest word.
    WordFocusEnter,
    /// Leave word focus mode, returning to scrolling (the word is remembered).
    WordFocusExit,
    /// Move the highlight to the previous word.
    WordFocusLeft,
    /// Move the highlight to the next word.
    WordFocusRight,
    /// Move up a line, landing on the word nearest the goal column.
    WordFocusUp,
    /// Move down a line, landing on the word nearest the goal column.
    WordFocusDown,
    // Application.
    OpenFile,
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
    ("caret_focus_enter", Command::CaretFocusEnter),
    ("caret_focus_exit", Command::CaretFocusExit),
    ("caret_focus_left", Command::CaretFocusLeft),
    ("caret_focus_right", Command::CaretFocusRight),
    ("caret_focus_up", Command::CaretFocusUp),
    ("caret_focus_down", Command::CaretFocusDown),
    ("caret_focus_next_word", Command::CaretFocusNextWord),
    ("caret_focus_end_word", Command::CaretFocusEndWord),
    ("caret_focus_prev_word", Command::CaretFocusPrevWord),
    ("line_focus_enter", Command::LineFocusEnter),
    ("line_focus_exit", Command::LineFocusExit),
    ("line_focus_left", Command::LineFocusLeft),
    ("line_focus_right", Command::LineFocusRight),
    ("line_focus_up", Command::LineFocusUp),
    ("line_focus_down", Command::LineFocusDown),
    ("word_focus_enter", Command::WordFocusEnter),
    ("word_focus_exit", Command::WordFocusExit),
    ("word_focus_left", Command::WordFocusLeft),
    ("word_focus_right", Command::WordFocusRight),
    ("word_focus_up", Command::WordFocusUp),
    ("word_focus_down", Command::WordFocusDown),
    ("open_file", Command::OpenFile),
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
        }
    }
}
