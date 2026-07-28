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
    /// Move to the end of the current word, whatever the scope.
    FocusEndWord,
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
    /// Move the active end to the end of the current word, whatever the scope.
    VisualEndWord,
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
    ("focus_end_word", Command::FocusEndWord),
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
    ("visual_end_word", Command::VisualEndWord),
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
