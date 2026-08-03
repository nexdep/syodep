//! Key chord and key sequence syntax.
//!
//! This is the textual key syntax shared by the config file and the Qt shell
//! (which encodes Qt key events into the same strings before forwarding them
//! to the core). The syntax is Vim-inspired:
//!
//! - Plain printable characters bind themselves: `j`, `G`, `+`.
//!   Case matters; `G` means shift+g and is written as the uppercase char.
//! - Special keys use angle brackets: `<Esc>`, `<CR>`, `<Tab>`, `<Space>`,
//!   `<Up>`, `<Down>`, `<Left>`, `<Right>`, `<PageUp>`, `<PageDown>`,
//!   `<Home>`, `<End>`, `<BS>`.
//! - Modifiers go inside the brackets: `<C-d>` (ctrl), `<A-x>` (alt),
//!   `<C-A-d>` (both). Shift on letters is expressed by case: `<C-G>`.
//! - A literal `<` is written bracketed — `<<>`, or `<C-<>` with ctrl —
//!   because a bare `<` opens a bracket group.
//! - A *sequence* is a concatenation of chords: `gg`, `zw`, `g<C-d>`.

use std::fmt;

/// A non-character key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Escape,
    Enter,
    Tab,
    Space,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
}

impl NamedKey {
    fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "esc" | "escape" => Self::Escape,
            "cr" | "enter" | "return" => Self::Enter,
            "tab" => Self::Tab,
            "space" => Self::Space,
            "bs" | "backspace" => Self::Backspace,
            "up" => Self::Up,
            "down" => Self::Down,
            "left" => Self::Left,
            "right" => Self::Right,
            "pageup" => Self::PageUp,
            "pagedown" => Self::PageDown,
            "home" => Self::Home,
            "end" => Self::End,
            _ => return None,
        })
    }

    fn canonical_name(self) -> &'static str {
        match self {
            Self::Escape => "Esc",
            Self::Enter => "CR",
            Self::Tab => "Tab",
            Self::Space => "Space",
            Self::Backspace => "BS",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::Home => "Home",
            Self::End => "End",
        }
    }
}

/// The key part of a chord: either a printable character or a named key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Named(NamedKey),
}

/// A single key press with modifiers.
///
/// Shift is intentionally absent: for characters it is encoded in the char
/// itself (`G` vs `g`), and named keys are matched shift-insensitively in
/// the first milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
}

impl Chord {
    pub fn char(c: char) -> Self {
        Self {
            key: Key::Char(c),
            ctrl: false,
            alt: false,
        }
    }

    pub fn named(named: NamedKey) -> Self {
        Self {
            key: Key::Named(named),
            ctrl: false,
            alt: false,
        }
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A literal '<' must render bracketed ("<<>"), or the output would
        // open a bracket group and fail to parse back.
        let needs_brackets = self.ctrl
            || self.alt
            || matches!(self.key, Key::Named(_))
            || matches!(self.key, Key::Char('<'));
        if needs_brackets {
            write!(f, "<")?;
            if self.ctrl {
                write!(f, "C-")?;
            }
            if self.alt {
                write!(f, "A-")?;
            }
        }
        match self.key {
            Key::Char(c) => write!(f, "{c}")?,
            Key::Named(n) => write!(f, "{}", n.canonical_name())?,
        }
        if needs_brackets {
            write!(f, ">")?;
        }
        Ok(())
    }
}

/// Error describing why a key-sequence string could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid key sequence {sequence:?}: {message}")]
pub struct KeyParseError {
    pub sequence: String,
    pub message: String,
}

/// The bracketed token that stands for the configured leader key.
pub const LEADER_TOKEN: &str = "leader";

/// Parse a full key sequence such as `gg`, `5j` is NOT valid here (counts are
/// runtime input, not bindings), `g<C-d>` is.
///
/// `<leader>` is *not* accepted here, because this function has no leader to
/// expand it to — use [`parse_sequence_with_leader`] for bindings. That is also
/// what stops a leader defined as `<leader>` from recursing: the leader itself is
/// parsed with this function.
pub fn parse_sequence(input: &str) -> Result<Vec<Chord>, KeyParseError> {
    parse_sequence_inner(input, None)
}

/// Parse a key sequence in which `<leader>` (any capitalisation) stands for
/// `leader`, the user's configured leader chords.
///
/// Expansion is a splice rather than a distinct chord kind: once a binding is in
/// the keymap it is an ordinary sequence, so the leader costs the input state
/// machine nothing and `<leader>w` disambiguates against `w` by the same
/// prefix rules as `gg` against `g`.
pub fn parse_sequence_with_leader(
    input: &str,
    leader: &[Chord],
) -> Result<Vec<Chord>, KeyParseError> {
    parse_sequence_inner(input, Some(leader))
}

fn parse_sequence_inner(
    input: &str,
    leader: Option<&[Chord]>,
) -> Result<Vec<Chord>, KeyParseError> {
    let err = |message: String| KeyParseError {
        sequence: input.to_owned(),
        message,
    };
    if input.is_empty() {
        return Err(err("empty key sequence".to_owned()));
    }
    let mut chords = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut inner = String::new();
            loop {
                match chars.next() {
                    Some('>') => break,
                    Some(c) => inner.push(c),
                    None => return Err(err(format!("unclosed '<' before end of {input:?}"))),
                }
            }
            if inner.eq_ignore_ascii_case(LEADER_TOKEN) {
                match leader {
                    Some(leader) => chords.extend_from_slice(leader),
                    None => {
                        return Err(err(
                            "<leader> is only allowed in keybindings; set the leader \
                             itself with [input] leader"
                                .to_owned(),
                        ))
                    }
                }
                continue;
            }
            chords.push(parse_bracketed(&inner).map_err(err)?);
        } else if c.is_whitespace() {
            return Err(err("whitespace is not allowed in key sequences".to_owned()));
        } else {
            chords.push(Chord::char(c));
        }
    }
    Ok(chords)
}

/// Parse the inside of a `<...>` group, e.g. `C-d`, `Esc`, `C-A-Left`.
fn parse_bracketed(inner: &str) -> Result<Chord, String> {
    let mut ctrl = false;
    let mut alt = false;
    let mut rest = inner;
    loop {
        if let Some(stripped) = rest.strip_prefix("C-").or_else(|| rest.strip_prefix("c-")) {
            if ctrl {
                return Err(format!("duplicate ctrl modifier in <{inner}>"));
            }
            ctrl = true;
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("A-").or_else(|| rest.strip_prefix("a-")) {
            if alt {
                return Err(format!("duplicate alt modifier in <{inner}>"));
            }
            alt = true;
            rest = stripped;
        } else {
            break;
        }
    }
    let key = if rest.chars().count() == 1 {
        Key::Char(rest.chars().next().unwrap())
    } else if let Some(named) = NamedKey::parse(rest) {
        Key::Named(named)
    } else {
        return Err(format!(
            "unknown key name {rest:?} in <{inner}> (expected a single character \
             or one of Esc, CR, Tab, Space, BS, Up, Down, Left, Right, PageUp, \
             PageDown, Home, End)"
        ));
    };
    Ok(Chord { key, ctrl, alt })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_char() {
        assert_eq!(parse_sequence("j").unwrap(), vec![Chord::char('j')]);
        assert_eq!(parse_sequence("G").unwrap(), vec![Chord::char('G')]);
        assert_eq!(parse_sequence("+").unwrap(), vec![Chord::char('+')]);
    }

    #[test]
    fn parses_multi_char_sequence() {
        assert_eq!(
            parse_sequence("gg").unwrap(),
            vec![Chord::char('g'), Chord::char('g')]
        );
        assert_eq!(
            parse_sequence("zw").unwrap(),
            vec![Chord::char('z'), Chord::char('w')]
        );
    }

    #[test]
    fn parses_ctrl_chord() {
        assert_eq!(
            parse_sequence("<C-d>").unwrap(),
            vec![Chord {
                key: Key::Char('d'),
                ctrl: true,
                alt: false
            }]
        );
    }

    #[test]
    fn parses_combined_modifiers_and_named_keys() {
        assert_eq!(
            parse_sequence("<C-A-Left>").unwrap(),
            vec![Chord {
                key: Key::Named(NamedKey::Left),
                ctrl: true,
                alt: true
            }]
        );
        assert_eq!(
            parse_sequence("<Esc>").unwrap(),
            vec![Chord::named(NamedKey::Escape)]
        );
    }

    #[test]
    fn parses_mixed_sequence() {
        assert_eq!(
            parse_sequence("g<C-d>").unwrap(),
            vec![
                Chord::char('g'),
                Chord {
                    key: Key::Char('d'),
                    ctrl: true,
                    alt: false
                }
            ]
        );
    }

    #[test]
    fn rejects_garbage_with_context() {
        let err = parse_sequence("<C-").unwrap_err();
        assert!(err.message.contains("unclosed"), "{err}");

        let err = parse_sequence("<Banana>").unwrap_err();
        assert!(err.message.contains("Banana"), "{err}");

        let err = parse_sequence("").unwrap_err();
        assert!(err.message.contains("empty"), "{err}");

        let err = parse_sequence("g g").unwrap_err();
        assert!(err.message.contains("whitespace"), "{err}");
    }

    #[test]
    fn leader_expands_to_its_chords() {
        let leader = parse_sequence("<Space>").unwrap();
        assert_eq!(
            parse_sequence_with_leader("<leader>w", &leader).unwrap(),
            vec![Chord::named(NamedKey::Space), Chord::char('w')]
        );
        // Capitalisation of the token does not matter, and a multi-chord leader
        // splices in whole.
        let long = parse_sequence("g<C-d>").unwrap();
        assert_eq!(
            parse_sequence_with_leader("<Leader>x", &long).unwrap(),
            vec![
                Chord::char('g'),
                Chord {
                    key: Key::Char('d'),
                    ctrl: true,
                    alt: false
                },
                Chord::char('x'),
            ]
        );
    }

    #[test]
    fn leader_is_rejected_where_there_is_none_to_expand() {
        // Which is what keeps `leader = "<leader>"` from recursing: the leader
        // string itself goes through `parse_sequence`.
        let err = parse_sequence("<leader>w").unwrap_err();
        assert!(err.message.contains("[input] leader"), "{err}");
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "j",
            "G",
            "gg",
            "<C-d>",
            "<Esc>",
            "<C-A-Left>",
            "g<C-d>",
            "<<>",
        ] {
            let chords = parse_sequence(s).unwrap();
            let rendered: String = chords.iter().map(|c| c.to_string()).collect();
            assert_eq!(parse_sequence(&rendered).unwrap(), chords, "for {s}");
        }
    }

    /// Regression: the shell encodes a pressed `<` as `<<>`, because a bare
    /// `<` opens a bracket group and is rejected as unclosed — the key would
    /// otherwise be silently swallowed and unbindable.
    #[test]
    fn literal_less_than_is_written_bracketed() {
        assert_eq!(parse_sequence("<<>").unwrap(), vec![Chord::char('<')]);
        assert_eq!(
            parse_sequence("<C-<>").unwrap(),
            vec![Chord {
                key: Key::Char('<'),
                ctrl: true,
                alt: false
            }]
        );
        assert_eq!(Chord::char('<').to_string(), "<<>");
        // A bare `<` stays an error rather than becoming a fourth spelling.
        let err = parse_sequence("<").unwrap_err();
        assert!(err.message.contains("unclosed"), "{err}");
    }
}
