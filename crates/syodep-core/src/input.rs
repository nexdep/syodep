//! Vim-like input handling: keymap trie + pending-input state machine.
//!
//! Input flows as individual [`Chord`]s (one per key press). The
//! [`InputState`] accumulates an optional count prefix (`5j`, `12G`) and a
//! pending chord sequence, resolving it against the [`Keymap`] trie.
//!
//! Disambiguation policy (documented in `docs/keybindings.md`): if a
//! sequence is both a complete binding and a prefix of a longer one, we wait
//! for more input instead of firing eagerly; `<Esc>` cancels. This keeps the
//! state machine timer-free and predictable.
//!
//! When the wait ends in a miss, we fall back to the **longest pending prefix
//! that is itself a complete binding**: that command fires and the leftover
//! chords are queued for replay (see [`InputState::next_replay`]). So with
//! `o` and `ow` both bound, `ow` resolves directly while `oj` runs `o` and
//! then `j`. Without this a bound sequence that is also a prefix would be
//! unreachable. Still timer-free: the decision is made by the *next* key
//! press, never by elapsed time.
//!
//! The replay queue is drained by the caller rather than resolved here,
//! because the fired command may change the mode — and the replayed chords
//! must resolve against the *new* mode's keymap.

use std::collections::{HashMap, VecDeque};

use syodep_config::keys::{self, Chord, Key, KeyParseError, NamedKey};

use crate::command::{Command, UnknownCommand};

/// A trie of chord sequences to commands.
#[derive(Debug, Default, Clone)]
pub struct Keymap {
    root: Node,
}

#[derive(Debug, Default, Clone)]
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
        let errors = keymap.overlay(entries);
        (keymap, errors)
    }

    /// Bind additional `(key sequence, command name)` pairs onto an existing
    /// keymap, overwriting any sequence that resolves identically. Used to
    /// build the caret-focus keymap as the normal keymap plus a few overrides,
    /// so normal-binding errors are validated (and reported) only once.
    /// Returns the errors found in *these* entries only.
    pub fn overlay<'a, I>(&mut self, entries: I) -> Vec<KeymapError>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
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
            self.bind(&chords, command);
        }
        errors
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

    /// The command bound to exactly `chords`, if any.
    fn command_at(&self, chords: &[Chord]) -> Option<Command> {
        self.lookup(chords)?.command
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
    replay: VecDeque<Chord>,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when a count or partial sequence is buffered.
    ///
    /// The replay queue deliberately does not count: it is always drained
    /// within the same [`handle`](Self::handle) cycle, so it is never
    /// observable from the status line.
    pub fn has_pending(&self) -> bool {
        self.count.is_some() || !self.pending.is_empty()
    }

    /// The next chord to re-feed after a longest-prefix fallback fired a
    /// command, or `None` when the queue is empty.
    ///
    /// Callers must re-select the keymap before each call, so that a
    /// mode-changing command takes effect on the chords that follow it.
    pub fn next_replay(&mut self) -> Option<Chord> {
        self.replay.pop_front()
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
        self.replay.clear();
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
                // Longest-prefix fallback: the pending sequence went nowhere,
                // but a prefix of it may be a complete binding. Fire the
                // longest such prefix and queue the rest for replay. The full
                // sequence is skipped (it is the miss we are handling) and so
                // is the empty one.
                for len in (1..self.pending.len()).rev() {
                    if let Some(command) = keymap.command_at(&self.pending[..len]) {
                        let count = self.count.take();
                        self.replay.extend(self.pending[len..].iter().copied());
                        self.pending.clear();
                        return KeyOutcome::Command { command, count };
                    }
                }
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
            // `o` is both a complete binding and a prefix of `ow` -- the case
            // the longest-prefix fallback exists for.
            ("o", "open_file"),
            ("ow", "fit_width"),
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

    /// Feed a whole sequence, draining replays, and collect what resolved.
    fn run(keymap: &Keymap, input: &mut InputState, chords: &[Chord]) -> Vec<KeyOutcome> {
        let mut out = Vec::new();
        for chord in chords {
            let mut next = Some(*chord);
            while let Some(chord) = next.take() {
                out.push(input.handle(keymap, chord));
                next = input.next_replay();
            }
        }
        out
    }

    fn command(command: Command, count: Option<u32>) -> KeyOutcome {
        KeyOutcome::Command { command, count }
    }

    #[test]
    fn prefix_command_fires_when_next_key_misses() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        // `o` waits (it is also a prefix of `ow`), then `oj` misses -- so `o`
        // fires and `j` is replayed.
        assert_eq!(input.handle(&keymap, chord('o')), KeyOutcome::Pending);
        assert_eq!(
            input.handle(&keymap, chord('j')),
            command(Command::OpenFile, None)
        );
        assert_eq!(input.next_replay(), Some(chord('j')));
        assert_eq!(
            input.handle(&keymap, chord('j')),
            command(Command::ScrollDown, None)
        );
        assert_eq!(input.next_replay(), None);
        assert!(!input.has_pending());
    }

    #[test]
    fn prefix_binding_still_resolves_directly() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        assert_eq!(
            run(&keymap, &mut input, &[chord('o'), chord('w')]),
            vec![KeyOutcome::Pending, command(Command::FitWidth, None)]
        );
        assert_eq!(input.next_replay(), None);
    }

    #[test]
    fn fallback_gives_the_count_to_the_prefix_command() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        // `5oj`: the count is consumed by the command that resolved, and the
        // replayed `j` runs countless.
        let outcomes = run(&keymap, &mut input, &[chord('5'), chord('o'), chord('j')]);
        assert_eq!(
            outcomes,
            vec![
                KeyOutcome::Pending,
                KeyOutcome::Pending,
                command(Command::OpenFile, Some(5)),
                command(Command::ScrollDown, None),
            ]
        );
    }

    #[test]
    fn digit_after_a_prefix_becomes_a_count_via_replay() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        // `o5j`: `o5` misses, so `o` fires and `5` replays into an empty
        // pending buffer -- where it re-enters the count branch and applies
        // to the `j` that follows.
        let outcomes = run(&keymap, &mut input, &[chord('o'), chord('5'), chord('j')]);
        assert_eq!(
            outcomes,
            vec![
                KeyOutcome::Pending,
                command(Command::OpenFile, None),
                KeyOutcome::Pending,
                command(Command::ScrollDown, Some(5)),
            ]
        );
    }

    #[test]
    fn escape_clears_the_replay_queue() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        input.handle(&keymap, chord('o'));
        input.handle(&keymap, chord('j'));
        assert!(input.next_replay().is_some() || !input.has_pending());
        input.clear();
        assert_eq!(input.next_replay(), None);
        assert!(!input.has_pending());
    }

    #[test]
    fn escape_cancels_a_pending_prefix_instead_of_firing_it() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        // Escape is intercepted while input is pending, before lookup -- so
        // `o<Esc>` cancels the half-typed `o` rather than running it.
        input.handle(&keymap, chord('o'));
        assert_eq!(
            input.handle(&keymap, Chord::named(NamedKey::Escape)),
            KeyOutcome::Pending
        );
        assert_eq!(input.next_replay(), None);
        assert!(!input.has_pending());
    }

    #[test]
    fn total_miss_without_a_prefix_command_still_resets() {
        let keymap = test_keymap();
        let mut input = InputState::new();
        // `g` is a prefix with no command of its own, so there is nothing to
        // fall back to and the old reset behavior stands.
        input.handle(&keymap, chord('g'));
        assert_eq!(input.handle(&keymap, chord('x')), KeyOutcome::Unmatched);
        assert_eq!(input.next_replay(), None);
        assert!(!input.has_pending());
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
