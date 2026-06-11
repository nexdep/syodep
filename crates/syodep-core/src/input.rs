//! Vim-like input handling: keymap trie + pending-input state machine.
//!
//! Input flows as individual [`Chord`]s (one per key press). The
//! [`InputState`] accumulates an optional count prefix (`5j`, `12G`) and a
//! pending chord sequence, resolving it against the [`Keymap`] trie.
//!
//! Disambiguation policy (documented in `docs/keybindings.md`): if a
//! sequence is both a complete binding and a prefix of a longer one, we wait
//! for more input instead of firing eagerly; `<Esc>` cancels. This keeps the
//! state machine timer-free and predictable. Defaults avoid such overlaps.

use std::collections::HashMap;

use syodep_config::keys::{self, Chord, Key, KeyParseError, NamedKey};

use crate::command::{Command, UnknownCommand};

/// A trie of chord sequences to commands.
#[derive(Debug, Default)]
pub struct Keymap {
    root: Node,
}

#[derive(Debug, Default)]
struct Node {
    command: Option<Command>,
    children: HashMap<Chord, Node>,
}

/// Errors found while building a keymap from config entries.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeymapError {
    #[error("{0}")]
    Key(#[from] KeyParseError),
    #[error("binding {sequence:?}: {source} (see docs/commands.md for the command list)")]
    Command {
        sequence: String,
        #[source]
        source: UnknownCommand,
    },
}

impl Keymap {
    /// Build a keymap from `(key sequence, command name)` pairs, e.g. the
    /// `[keys]` table of the config file.
    ///
    /// All entries are validated; every invalid entry is reported (not just
    /// the first), so users can fix their config in one pass. Valid entries
    /// are kept even when others fail.
    pub fn from_entries<'a, I>(entries: I) -> (Self, Vec<KeymapError>)
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut keymap = Self::default();
        let mut errors = Vec::new();
        for (sequence, command_name) in entries {
            let chords = match keys::parse_sequence(sequence) {
                Ok(chords) => chords,
                Err(e) => {
                    errors.push(KeymapError::Key(e));
                    continue;
                }
            };
            let command = match command_name.parse::<Command>() {
                Ok(command) => command,
                Err(source) => {
                    errors.push(KeymapError::Command {
                        sequence: sequence.to_owned(),
                        source,
                    });
                    continue;
                }
            };
            keymap.bind(&chords, command);
        }
        (keymap, errors)
    }

    fn bind(&mut self, chords: &[Chord], command: Command) {
        let mut node = &mut self.root;
        for chord in chords {
            node = node.children.entry(*chord).or_default();
        }
        node.command = Some(command);
    }

    fn lookup(&self, chords: &[Chord]) -> Option<&Node> {
        let mut node = &self.root;
        for chord in chords {
            node = node.children.get(chord)?;
        }
        Some(node)
    }
}

/// Result of feeding one chord into [`InputState::handle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    /// The chord extended a pending count or sequence; wait for more input.
    Pending,
    /// A binding resolved.
    Command {
        command: Command,
        count: Option<u32>,
    },
    /// The sequence matched nothing; pending state was reset.
    Unmatched,
}

/// Accumulates count prefixes and chord sequences between key presses.
#[derive(Debug, Default)]
pub struct InputState {
    count: Option<u32>,
    pending: Vec<Chord>,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when a count or partial sequence is buffered.
    pub fn has_pending(&self) -> bool {
        self.count.is_some() || !self.pending.is_empty()
    }

    /// Human-readable pending input for the status line, e.g. `12g`.
    pub fn pending_display(&self) -> String {
        let mut out = String::new();
        if let Some(count) = self.count {
            out.push_str(&count.to_string());
        }
        for chord in &self.pending {
            out.push_str(&chord.to_string());
        }
        out
    }

    pub fn clear(&mut self) {
        self.count = None;
        self.pending.clear();
    }

    /// Feed one key press through the state machine.
    pub fn handle(&mut self, keymap: &Keymap, chord: Chord) -> KeyOutcome {
        // Escape always clears buffered input first; only a bare Escape
        // reaches the keymap (where it is bound to `cancel` by default).
        if chord == Chord::named(NamedKey::Escape) && self.has_pending() {
            self.clear();
            return KeyOutcome::Pending;
        }

        // Digits build up a count prefix, unless the digit itself starts a
        // bound sequence (so users may bind digits if they want). `0` only
        // counts when a count is already in progress, mirroring Vim where
        // a leading 0 is a motion.
        if self.pending.is_empty() {
            if let Key::Char(c) = chord.key {
                if let Some(digit) = c.to_digit(10) {
                    let starts_binding =
                        !chord.ctrl && !chord.alt && keymap.lookup(&[chord]).is_some();
                    let leading_zero = digit == 0 && self.count.is_none();
                    if !chord.ctrl && !chord.alt && !starts_binding && !leading_zero {
                        self.count = Some(
                            self.count
                                .unwrap_or(0)
                                .saturating_mul(10)
                                .saturating_add(digit),
                        );
                        return KeyOutcome::Pending;
                    }
                }
            }
        }

        self.pending.push(chord);
        match keymap.lookup(&self.pending) {
            None => {
                self.clear();
                KeyOutcome::Unmatched
            }
            Some(node) => {
                if let Some(command) = node.command {
                    if node.children.is_empty() {
                        let count = self.count.take();
                        self.pending.clear();
                        return KeyOutcome::Command { command, count };
                    }
                    // Both a complete binding and a prefix of a longer one:
                    // wait for more input (see module docs).
                }
                KeyOutcome::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_keymap() -> Keymap {
        let (keymap, errors) = Keymap::from_entries([
            ("j", "scroll_down"),
            ("k", "scroll_up"),
            ("gg", "goto_first_page"),
            ("G", "goto_last_page"),
            ("<C-d>", "scroll_half_page_down"),
            ("zw", "fit_width"),
            ("z0", "zoom_reset"),
            ("<Esc>", "cancel"),
        ]);
        assert!(errors.is_empty(), "{errors:?}");
        keymap
    }

    fn chord(c: char) -> Chord {
        Chord::char(c)
    }

    #[test]
    fn single_key_resolves() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        assert_eq!(
            input.handle(&keymap, chord('j')),
            KeyOutcome::Command {
                command: Command::ScrollDown,
                count: None
            }
        );
        assert!(!input.has_pending());
    }

    #[test]
    fn count_prefix_applies() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        assert_eq!(input.handle(&keymap, chord('5')), KeyOutcome::Pending);
        assert_eq!(
            input.handle(&keymap, chord('j')),
            KeyOutcome::Command {
                command: Command::ScrollDown,
                count: Some(5)
            }
        );
    }

    #[test]
    fn multi_digit_count() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        input.handle(&keymap, chord('1'));
        input.handle(&keymap, chord('2'));
        input.handle(&keymap, chord('0'));
        assert_eq!(
            input.handle(&keymap, chord('G')),
            KeyOutcome::Command {
                command: Command::GotoLastPage,
                count: Some(120)
            }
        );
    }

    #[test]
    fn multi_key_sequence_resolves() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        assert_eq!(input.handle(&keymap, chord('g')), KeyOutcome::Pending);
        assert!(input.has_pending());
        assert_eq!(
            input.handle(&keymap, chord('g')),
            KeyOutcome::Command {
                command: Command::GotoFirstPage,
                count: None
            }
        );
    }

    #[test]
    fn sequences_sharing_prefix_disambiguate() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        input.handle(&keymap, chord('z'));
        assert_eq!(
            input.handle(&keymap, chord('0')),
            KeyOutcome::Command {
                command: Command::ZoomReset,
                count: None
            }
        );
        input.handle(&keymap, chord('z'));
        assert_eq!(
            input.handle(&keymap, chord('w')),
            KeyOutcome::Command {
                command: Command::FitWidth,
                count: None
            }
        );
    }

    #[test]
    fn unmatched_sequence_resets() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        input.handle(&keymap, chord('g'));
        assert_eq!(input.handle(&keymap, chord('x')), KeyOutcome::Unmatched);
        assert!(!input.has_pending());
        // State machine still works afterwards.
        assert_eq!(
            input.handle(&keymap, chord('j')),
            KeyOutcome::Command {
                command: Command::ScrollDown,
                count: None
            }
        );
    }

    #[test]
    fn escape_clears_pending_input() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        input.handle(&keymap, chord('4'));
        input.handle(&keymap, chord('g'));
        assert!(input.has_pending());
        assert_eq!(
            input.handle(&keymap, Chord::named(NamedKey::Escape)),
            KeyOutcome::Pending
        );
        assert!(!input.has_pending());
        // A bare Escape resolves to cancel.
        assert_eq!(
            input.handle(&keymap, Chord::named(NamedKey::Escape)),
            KeyOutcome::Command {
                command: Command::Cancel,
                count: None
            }
        );
    }

    #[test]
    fn ctrl_chord_resolves() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        let ctrl_d = Chord {
            key: Key::Char('d'),
            ctrl: true,
            alt: false,
        };
        assert_eq!(
            input.handle(&keymap, ctrl_d),
            KeyOutcome::Command {
                command: Command::ScrollHalfPageDown,
                count: None
            }
        );
    }

    #[test]
    fn leading_zero_is_not_a_count() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        // '0' is unbound in the test keymap and no count is pending, so it
        // falls through to sequence matching and misses.
        assert_eq!(input.handle(&keymap, chord('0')), KeyOutcome::Unmatched);
    }

    #[test]
    fn pending_display_shows_count_and_sequence() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        input.handle(&keymap, chord('1'));
        input.handle(&keymap, chord('2'));
        input.handle(&keymap, chord('g'));
        assert_eq!(input.pending_display(), "12g");
    }

    #[test]
    fn keymap_reports_all_errors_but_keeps_valid_entries() {
        let (keymap, errors) = Keymap::from_entries([
            ("j", "scroll_down"),
            ("<Oops>", "scroll_up"),
            ("k", "not_a_command"),
        ]);
        assert_eq!(errors.len(), 2);
        let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        assert!(messages.iter().any(|m| m.contains("Oops")), "{messages:?}");
        assert!(
            messages.iter().any(|m| m.contains("not_a_command")),
            "{messages:?}"
        );
        let mut input = InputState::new();
        assert_eq!(
            input.handle(&keymap, chord('j')),
            KeyOutcome::Command {
                command: Command::ScrollDown,
                count: None
            }
        );
    }
}
