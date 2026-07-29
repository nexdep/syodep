//! Configuration loading for syodep.
//!
//! Configuration lives in a single human-editable TOML file. This crate is
//! responsible for the *shape* of the config (sections, types, key-chord
//! syntax). Semantic validation of command names happens in `syodep-core`,
//! which owns the command set.
//!
//! Design rule: invalid configuration must never abort the application. The
//! loader returns either a parsed [`Config`] or a [`ConfigError`] with a
//! message good enough to fix the file; callers fall back to
//! [`Config::default`] and surface the error to the user.

pub mod keys;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Top-level configuration, mirroring the TOML file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub view: ViewConfig,
    /// Raw keybindings: key-sequence string -> command-name string.
    /// Parsed and validated into a keymap by `syodep-core`.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// Focus-mode keybindings (the `[focus_keys]` table). These overlay the
    /// normal `keys` while focus mode is active, so `hjkl`/`<Esc>` can mean
    /// something different there while every other binding still works.
    ///
    /// One table covers every scope: the motion commands dispatch on the active
    /// scope, so there is nothing scope-specific left to bind.
    #[serde(default)]
    pub focus_keys: BTreeMap<String, String>,
    /// Visual-mode keybindings (the `[visual_keys]` table). These overlay the
    /// normal `keys` while visual mode is active.
    #[serde(default)]
    pub visual_keys: BTreeMap<String, String>,
    /// `[files]` section: file-dialog and path behaviour.
    #[serde(default)]
    pub files: FilesConfig,
    /// `[input]` section: key-sequence timing.
    #[serde(default)]
    pub input: InputConfig,
}

/// `[input]` section: how long a partial key sequence waits.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct InputConfig {
    /// Milliseconds a partial sequence waits before it resolves on its own.
    ///
    /// This is what lets a key that is both a binding and a prefix — `c`, `v`,
    /// or `o` in visual mode — be used on its own: press it, pause, and it
    /// acts. Sequences typed at normal speed never reach the pause. `0`
    /// disables it, restoring "only the next key press ends the wait".
    pub timeout_ms: u32,
}

impl Default for InputConfig {
    fn default() -> Self {
        // Long enough that ordinary two-key chords never trip it, short enough
        // that a deliberate pause feels immediate.
        Self { timeout_ms: 500 }
    }
}

/// `[files]` section: file-dialog and path behaviour.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(deny_unknown_fields, default)]
pub struct FilesConfig {
    /// Starting directory for the Open dialog. When unset, or pointing at a
    /// path that is not an existing directory, syodep uses the launch
    /// (current working) directory instead.
    pub open_dir: Option<String>,
}

/// `[view]` section: rendering and navigation tunables.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ViewConfig {
    /// Vertical pixels moved by one scroll step (`scroll_down` / `scroll_up`).
    pub scroll_step: f32,
    /// Horizontal pixels moved by one scroll step (`scroll_left` / `scroll_right`).
    pub horizontal_scroll_step: f32,
    /// Gap between pages in document points (1/72 inch at zoom 1.0).
    pub page_gap: f32,
    /// Initial zoom factor for documents without a saved position.
    pub default_zoom: f32,
    /// If true, fit page width to the window when opening a document
    /// without a saved position (overrides `default_zoom`).
    pub fit_width_on_open: bool,
    /// Multiplicative step for `zoom_in` / `zoom_out`.
    pub zoom_step: f32,
    /// Canvas background color as `#rrggbb`.
    pub background: String,
    /// Highlight color for focus mode as `#rrggbb`. One colour for every
    /// scope on purpose: the useful signal is focus vs selection, not which
    /// granularity is active.
    pub focus_color: String,
    /// Opacity of the focus highlight, 0.0 (invisible) to 1.0 (opaque).
    pub focus_opacity: f32,
    /// Highlight color for the visual-mode selection as `#rrggbb`.
    pub visual_color: String,
    /// Opacity of the selection highlight, 0.0 (invisible) to 1.0 (opaque).
    pub visual_opacity: f32,
    /// Detect tables so each one is a single stop for focus and selection
    /// motions above char scope. Costs a second text-extraction pass per page
    /// and relies on a heuristic, so it can be turned off.
    pub detect_tables: bool,
    /// Detect headings so each is a single step at sentence and paragraph
    /// scope. Costs nothing extra to extract, but relies on a heuristic.
    pub detect_headings: bool,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            scroll_step: 60.0,
            horizontal_scroll_step: 60.0,
            page_gap: 12.0,
            default_zoom: 1.0,
            fit_width_on_open: true,
            zoom_step: 1.1,
            background: "#1e1e1e".to_owned(),
            focus_color: "#add8e6".to_owned(),
            focus_opacity: 0.4,
            visual_color: "#d3d3d3".to_owned(),
            visual_opacity: 0.4,
            detect_tables: true,
            detect_headings: true,
        }
    }
}

/// Parse `#rrggbb` into its components. Returns `None` for anything else --
/// wrong length, missing `#`, non-hex digits -- so the caller can fall back to
/// a default and warn rather than rendering an invisible overlay.
pub fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let component = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((component(0)?, component(2)?, component(4)?))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            view: ViewConfig::default(),
            keys: default_keybindings(),
            focus_keys: default_focus_keybindings(),
            visual_keys: default_visual_keybindings(),
            files: FilesConfig::default(),
            input: InputConfig::default(),
        }
    }
}

/// Built-in keybindings, used when the config file has no `[keys]` section.
///
/// Every entry here must be documented in `docs/keybindings.md`.
pub fn default_keybindings() -> BTreeMap<String, String> {
    [
        ("j", "scroll_down"),
        ("k", "scroll_up"),
        ("h", "scroll_left"),
        ("l", "scroll_right"),
        ("<Down>", "scroll_down"),
        ("<Up>", "scroll_up"),
        ("<Left>", "scroll_left"),
        ("<Right>", "scroll_right"),
        ("J", "next_page"),
        ("K", "prev_page"),
        ("<PageDown>", "next_page"),
        ("<PageUp>", "prev_page"),
        ("<C-d>", "scroll_half_page_down"),
        ("<C-u>", "scroll_half_page_up"),
        ("<C-f>", "scroll_page_down"),
        ("<C-b>", "scroll_page_up"),
        ("gg", "goto_first_page"),
        ("G", "goto_last_page"),
        ("+", "zoom_in"),
        ("=", "zoom_in"),
        ("-", "zoom_out"),
        ("zw", "fit_width"),
        ("z0", "zoom_reset"),
        // `c` plus a scope letter focuses at that granularity. The same
        // bindings work *inside* focus mode, where they change the scope
        // without moving the highlight.
        // `c` alone enters focus keeping the current scope, once the pause
        // resolves it; `c` plus a scope letter names the granularity.
        ("c", "focus_enter"),
        ("cc", "focus_enter_char"),
        ("ce", "focus_enter_line"),
        ("cw", "focus_enter_word"),
        ("cs", "focus_enter_sentence"),
        ("cp", "focus_enter_paragraph"),
        // `v` alone inherits the focus scope; `v` plus a scope letter names
        // it. `v` is a binding *and* a prefix, which the input
        // state machine resolves by longest-prefix fallback.
        ("v", "visual_enter"),
        ("vc", "visual_enter_char"),
        ("vw", "visual_enter_word"),
        ("ve", "visual_enter_line"),
        ("vs", "visual_enter_sentence"),
        ("vp", "visual_enter_paragraph"),
        // Ctrl rather than a bare `o`: visual mode binds `o` to swapping the
        // selection ends, and a command should not vanish in one mode.
        ("<C-o>", "open_file"),
        ("q", "quit"),
        ("<Esc>", "cancel"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Built-in focus-mode keybindings (the `[focus_keys]` table). These overlay
/// the normal bindings while focus mode is active: `hjkl`/arrows move the
/// highlight by one unit of the active scope, `w`/`e`/`b` move a word at a time
/// whatever the scope, and `<Esc>` leaves the mode; everything else keeps its
/// normal meaning.
///
/// The shape deliberately mirrors [`default_visual_keybindings`]: the same keys
/// do the same things to one position that they do to the moving end of a
/// selection. Scope-specific meanings (line scope's `h`/`l` jumping columns,
/// sentence and paragraph collapsing all four directions to previous/next) come
/// from the commands, not from the bindings.
///
/// Every entry here must be documented in `docs/keybindings.md`.
pub fn default_focus_keybindings() -> BTreeMap<String, String> {
    [
        ("h", "focus_left"),
        ("j", "focus_down"),
        ("k", "focus_up"),
        ("l", "focus_right"),
        ("<Left>", "focus_left"),
        ("<Down>", "focus_down"),
        ("<Up>", "focus_up"),
        ("<Right>", "focus_right"),
        ("w", "focus_next_word"),
        ("e", "focus_end_word"),
        ("b", "focus_prev_word"),
        // Sentence and paragraph get a forward motion each, the same way `w`
        // is the word one. `cs`/`cp` still switch scope: `s` *moves* by a
        // sentence, `cs` *focuses by* sentence.
        ("s", "focus_next_sentence"),
        ("p", "focus_next_paragraph"),
        ("<Esc>", "focus_exit"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Built-in visual-mode keybindings (the `[visual_keys]` table).
///
/// `v` acts on the end that is moving, `o` on the other one: `vw` makes the
/// active end word-granular, `ow` switches ends and makes *that* one word
/// granular. Both are bindings and prefixes, resolved by the input state
/// machine's longest-prefix fallback.
///
/// Every entry here must be documented in `docs/keybindings.md`.
pub fn default_visual_keybindings() -> BTreeMap<String, String> {
    [
        ("h", "visual_left"),
        ("j", "visual_down"),
        ("k", "visual_up"),
        ("l", "visual_right"),
        ("<Left>", "visual_left"),
        ("<Down>", "visual_down"),
        ("<Up>", "visual_up"),
        ("<Right>", "visual_right"),
        ("w", "visual_next_word"),
        ("e", "visual_end_word"),
        ("b", "visual_prev_word"),
        ("s", "visual_next_sentence"),
        ("p", "visual_next_paragraph"),
        ("o", "visual_swap_ends"),
        // `o` waits for the next key (it is also a prefix), so it only takes
        // effect together with whatever follows. `oo` swaps immediately.
        ("oo", "visual_swap_ends"),
        ("oc", "visual_other_char"),
        ("ow", "visual_other_word"),
        ("oe", "visual_other_line"),
        ("os", "visual_other_sentence"),
        ("op", "visual_other_paragraph"),
        ("v", "visual_exit"),
        ("vc", "visual_scope_char"),
        ("vw", "visual_scope_word"),
        ("ve", "visual_scope_line"),
        ("vs", "visual_scope_sentence"),
        ("vp", "visual_scope_paragraph"),
        ("<Esc>", "visual_exit"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Errors produced while loading or parsing configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config file {path}: {message}")]
    Parse { path: String, message: String },
}

/// Tables that existed before the five focus modes collapsed into one.
const REMOVED_FOCUS_TABLES: [&str; 5] = [
    "caret_focus_keys",
    "line_focus_keys",
    "word_focus_keys",
    "sentence_focus_keys",
    "paragraph_focus_keys",
];

/// Append a migration hint when a parse failure looks like a pre-0.7 config.
///
/// `deny_unknown_fields` rejects the *whole* file, so a config still naming
/// `[word_focus_keys]` loses `[view]`, `[files]` and everything else — and the
/// bare serde message ("unknown field `word_focus_keys`") does not say what to
/// do about it. The clean break stands; this just makes it legible.
fn migration_hint(text: &str, message: String) -> String {
    let stale: Vec<&str> = REMOVED_FOCUS_TABLES
        .iter()
        .copied()
        .filter(|table| text.contains(&format!("[{table}]")))
        .collect();
    if stale.is_empty() {
        return message;
    }
    format!(
        "{message}\n\
         hint: the five focus modes are now one mode with a scope, so {} \
         became a single [focus_keys] table. Rename it and replace the \
         per-scope command names ({}_left, ...) with focus_left / focus_right \
         / focus_up / focus_down. See docs/config.md.",
        stale
            .iter()
            .map(|t| format!("[{t}]"))
            .collect::<Vec<_>>()
            .join(", "),
        stale[0].trim_end_matches("_keys"),
    )
}

impl Config {
    /// Parse a configuration from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let mut config: Config =
            toml::from_str(text).map_err(|e| migration_hint(text, e.to_string()))?;
        // An empty or missing [keys] table means "use the defaults". Users who
        // want extra bindings list only their additions; defaults still apply
        // unless explicitly rebound.
        let mut keys = default_keybindings();
        keys.extend(std::mem::take(&mut config.keys));
        config.keys = keys;
        let mut focus_keys = default_focus_keybindings();
        focus_keys.extend(std::mem::take(&mut config.focus_keys));
        config.focus_keys = focus_keys;
        let mut visual_keys = default_visual_keybindings();
        visual_keys.extend(std::mem::take(&mut config.visual_keys));
        config.visual_keys = visual_keys;
        Ok(config)
    }

    /// Load configuration from `path`.
    ///
    /// A missing file is not an error: defaults are returned so a fresh
    /// install works without any setup.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(ConfigError::Io {
                    path: path.display().to_string(),
                    source: e,
                })
            }
        };
        Self::from_toml(&text).map_err(|message| ConfigError::Parse {
            path: path.display().to_string(),
            message,
        })
    }
}

/// Render a documented `config.toml` with every option set to its built-in
/// default, suitable for `syodep --defaults`.
///
/// All values come from [`Config::default`], [`ViewConfig::default`] and the
/// `default_*_keybindings` functions, so the output can never drift from the
/// real defaults. Keybindings are emitted as active lines (each equal to its
/// default) so the file doubles as a complete reference; copying it verbatim to
/// the config path is a no-op relative to the built-in behaviour.
pub fn default_config_doc() -> String {
    let view = ViewConfig::default();
    let mut out = String::new();

    out.push_str(
        "# syodep configuration — generated by `syodep --defaults`.\n\
         #\n\
         # Every option below is set to its built-in default. Copy this file to:\n\
         #   Linux:   ~/.config/syodep/config.toml\n\
         #   Windows: %APPDATA%\\syodep\\config.toml\n\
         #\n\
         # Everything here is optional; deleting a value falls back to the same\n\
         # default shown. An invalid file never prevents syodep from starting: the\n\
         # error is shown in the status bar and defaults are used instead.\n\
         # See docs/config.md for the full reference.\n\n",
    );

    out.push_str("[view]\n");
    out.push_str("# Pixels moved per scroll step (j / k).\n");
    let _ = writeln!(out, "scroll_step = {}", float(view.scroll_step));
    out.push_str("# Pixels moved per horizontal scroll step (h / l).\n");
    let _ = writeln!(
        out,
        "horizontal_scroll_step = {}",
        float(view.horizontal_scroll_step)
    );
    out.push_str("# Gap between pages, in PDF points (1/72 inch at 100% zoom).\n");
    let _ = writeln!(out, "page_gap = {}", float(view.page_gap));
    out.push_str("# Zoom factor used when opening a document without a saved position...\n");
    let _ = writeln!(out, "default_zoom = {}", float(view.default_zoom));
    out.push_str(
        "# ...unless this is true, in which case the page is fitted to the window width.\n",
    );
    let _ = writeln!(out, "fit_width_on_open = {}", view.fit_width_on_open);
    out.push_str("# Multiplicative zoom step for zoom_in / zoom_out.\n");
    let _ = writeln!(out, "zoom_step = {}", float(view.zoom_step));
    out.push_str("# Canvas background color (#rrggbb).\n");
    let _ = writeln!(out, "background = \"{}\"", view.background);
    out.push_str(
        "# Highlight for focus mode. Every scope shares one colour, so the\n\
         # highlight tells you that focus is active rather than which\n\
         # granularity you are in.\n",
    );
    let _ = writeln!(out, "focus_color = \"{}\"", view.focus_color);
    out.push_str("# Opacity of the focus highlight, 0.0 to 1.0.\n");
    let _ = writeln!(out, "focus_opacity = {}", float(view.focus_opacity));
    out.push_str("# Highlight for the visual-mode selection (#rrggbb).\n");
    let _ = writeln!(out, "visual_color = \"{}\"", view.visual_color);
    out.push_str("# Opacity of the selection highlight, 0.0 to 1.0.\n");
    let _ = writeln!(out, "visual_opacity = {}", float(view.visual_opacity));
    out.push_str(
        "# Treat each detected table as one stop for focus and selection\n\
         # motions (char scope still steps through a table's characters).\n\
         # Images are always single stops; this only controls table detection,\n\
         # which costs a second text pass per page.\n",
    );
    let _ = writeln!(out, "detect_tables = {}", view.detect_tables);
    out.push_str(
        "# Treat each heading as one step at sentence and paragraph scope,\n\
         # so it is not glued to the text below it for want of a full stop.\n\
         # Word and line scope still move through a heading normally.\n",
    );
    let _ = writeln!(out, "detect_headings = {}", view.detect_headings);
    out.push('\n');

    out.push_str(
        "[files]\n\
         # Starting directory for the Open dialog (<C-o>). When unset, the\n\
         # dialog opens in the directory syodep was launched from. If the path below\n\
         # does not exist (or is not a directory), syodep falls back to the launch\n\
         # directory. Run `syodep --check` to see which directory is in effect.\n\
         # Use an absolute path (\"~\" is not expanded). Unset by default:\n\
         # open_dir = \"/home/me/papers\"\n\n",
    );

    out.push_str(
        "[input]\n\
         # How long (milliseconds) a half-typed key sequence waits before it acts\n\
         # on its own. This is what lets a key that is both a command and the\n\
         # start of a longer one -- \"c\", \"v\", or \"o\" while selecting -- be used\n\
         # by itself: press it, pause, and it acts. Sequences typed at normal\n\
         # speed never reach the pause. Set to 0 to switch it off, so only the\n\
         # next key press ever ends the wait.\n",
    );
    let _ = writeln!(out, "timeout_ms = {}", InputConfig::default().timeout_ms);
    out.push('\n');

    out.push_str(
        "# Keybindings: \"key sequence\" = \"command\". The entries below are the\n\
         # built-in defaults; in your own config you only need to list changes,\n\
         # which ADD TO or OVERRIDE these.\n\
         # Key syntax (see docs/keybindings.md): plain chars (\"j\", \"G\", \"+\"),\n\
         # sequences (\"gg\", \"zw\"), special keys in angle brackets (\"<Esc>\", \"<CR>\",\n\
         # \"<Space>\", \"<Up>\", \"<PageDown>\", ...), modifiers inside the brackets\n\
         # (\"<C-d>\" = ctrl+d, \"<A-x>\" = alt+x, \"<C-A-Left>\").\n\
         # Command names are listed in docs/commands.md.\n",
    );
    push_keytable(&mut out, "keys", &default_keybindings());

    out.push_str(
        "\n# Focus-mode keybindings (active after pressing \"cc\", \"cw\", \"ce\", \"cs\" or\n\
         # \"cp\"). These overlay the normal [keys] while focus mode is active: hjkl and\n\
         # the arrows move the highlight by one unit of the active scope, w/e/b move a\n\
         # word at a time whatever the scope, and <Esc> exits.\n\
         # One table covers every scope, because the commands dispatch on the scope:\n\
         # \"focus_left\" is a character in char scope, a word in word scope, a column\n\
         # jump in line scope and the previous unit in sentence/paragraph scope.\n",
    );
    push_keytable(&mut out, "focus_keys", &default_focus_keybindings());

    out.push_str(
        "\n# Visual-mode keybindings (active after pressing \"v\"). hjkl/arrows grow the\n\
         # selection by one unit of the active end's scope. \"v\" plus a scope letter\n\
         # changes the moving end's scope, \"o\" switches ends and \"o\" plus a scope\n\
         # letter does both. <Esc> exits to the mode visual mode was entered from.\n",
    );
    push_keytable(&mut out, "visual_keys", &default_visual_keybindings());

    out
}

/// Emit a `[table]` header followed by every binding as a quoted active line.
/// The map is already sorted (`BTreeMap`), giving stable output.
fn push_keytable(out: &mut String, table: &str, bindings: &BTreeMap<String, String>) {
    let _ = writeln!(out, "[{table}]");
    for (key, command) in bindings {
        let _ = writeln!(out, "\"{key}\" = \"{command}\"");
    }
}

/// Format an `f32` default so it always keeps a decimal point (`60` -> `60.0`),
/// matching TOML float syntax and the hand-written reference sample.
fn float(value: f32) -> String {
    let s = value.to_string();
    if s.contains('.') {
        s
    } else {
        format!("{s}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_hex_color("#add8e6"), Some((0xad, 0xd8, 0xe6)));
        assert_eq!(parse_hex_color("#000000"), Some((0, 0, 0)));
        assert_eq!(parse_hex_color("#FFFFFF"), Some((255, 255, 255)));
    }

    #[test]
    fn rejects_malformed_hex_colors() {
        assert_eq!(parse_hex_color("add8e6"), None, "missing #");
        assert_eq!(parse_hex_color("#add8e"), None, "too short");
        assert_eq!(parse_hex_color("#add8e6f"), None, "too long");
        assert_eq!(parse_hex_color("#gggggg"), None, "not hex");
        assert_eq!(parse_hex_color(""), None);
        assert_eq!(parse_hex_color("#"), None);
        // 8-digit hex is rejected rather than silently dropping the alpha:
        // opacity is a separate option.
        assert_eq!(parse_hex_color("#add8e6ff"), None);
    }

    #[test]
    fn overlay_colors_default_and_user_override() {
        // r##"..."## because the TOML contains `"#`, which would close an
        // r#"..."# literal early.
        let config = Config::from_toml(
            r##"
            [view]
            focus_color = "#112233"
            visual_opacity = 0.75
            "##,
        )
        .unwrap();
        assert_eq!(config.view.focus_color, "#112233");
        assert_eq!(config.view.visual_opacity, 0.75);
        // Untouched colour options keep their defaults.
        assert_eq!(config.view.visual_color, "#d3d3d3");
        assert_eq!(config.view.focus_opacity, 0.4);
        assert_eq!(config.view.background, "#1e1e1e");
    }

    #[test]
    fn default_config_has_sane_view_settings() {
        let config = Config::default();
        assert!(config.view.scroll_step > 0.0);
        assert!(config.view.zoom_step > 1.0);
        assert!(config.view.fit_width_on_open);
    }

    #[test]
    fn default_keybindings_cover_core_navigation() {
        let keys = default_keybindings();
        assert_eq!(keys.get("j").map(String::as_str), Some("scroll_down"));
        assert_eq!(keys.get("gg").map(String::as_str), Some("goto_first_page"));
        assert_eq!(keys.get("G").map(String::as_str), Some("goto_last_page"));
    }

    #[test]
    fn parses_view_section() {
        let config = Config::from_toml(
            r#"
            [view]
            scroll_step = 120.0
            default_zoom = 1.5
            fit_width_on_open = false
            "#,
        )
        .unwrap();
        assert_eq!(config.view.scroll_step, 120.0);
        assert_eq!(config.view.default_zoom, 1.5);
        assert!(!config.view.fit_width_on_open);
        // Unspecified fields keep defaults.
        assert_eq!(config.view.page_gap, ViewConfig::default().page_gap);
    }

    #[test]
    fn user_keys_extend_and_override_defaults() {
        let config = Config::from_toml(
            r#"
            [keys]
            "j" = "scroll_half_page_down"
            "<C-o>" = "quit"
            "#,
        )
        .unwrap();
        // Overridden.
        assert_eq!(
            config.keys.get("j").map(String::as_str),
            Some("scroll_half_page_down")
        );
        // Added.
        assert_eq!(config.keys.get("<C-o>").map(String::as_str), Some("quit"));
        // Untouched default survives.
        assert_eq!(config.keys.get("k").map(String::as_str), Some("scroll_up"));
    }

    #[test]
    fn focus_keys_default_and_user_override() {
        let config = Config::from_toml(
            r#"
            [focus_keys]
            "y" = "focus_right"
            "#,
        )
        .unwrap();
        // Built-in focus bindings survive.
        assert_eq!(
            config.focus_keys.get("h").map(String::as_str),
            Some("focus_left")
        );
        assert_eq!(
            config.focus_keys.get("j").map(String::as_str),
            Some("focus_down")
        );
        assert_eq!(
            config.focus_keys.get("e").map(String::as_str),
            Some("focus_end_word")
        );
        assert_eq!(
            config.focus_keys.get("b").map(String::as_str),
            Some("focus_prev_word")
        );
        assert_eq!(
            config.focus_keys.get("<Esc>").map(String::as_str),
            Some("focus_exit")
        );
        // User override is merged in.
        assert_eq!(
            config.focus_keys.get("y").map(String::as_str),
            Some("focus_right")
        );
        // The enter bindings live in the normal table, one per scope. They are
        // also what changes the scope from inside focus mode.
        for (key, command) in [
            ("cc", "focus_enter_char"),
            ("cw", "focus_enter_word"),
            ("ce", "focus_enter_line"),
            ("cs", "focus_enter_sentence"),
            ("cp", "focus_enter_paragraph"),
        ] {
            assert_eq!(config.keys.get(key).map(String::as_str), Some(command));
        }
    }

    #[test]
    fn visual_keys_default_and_user_override() {
        let config = Config::from_toml(
            r#"
            [visual_keys]
            "y" = "visual_exit"
            "#,
        )
        .unwrap();
        // Built-in visual bindings survive.
        assert_eq!(
            config.visual_keys.get("h").map(String::as_str),
            Some("visual_left")
        );
        assert_eq!(
            config.visual_keys.get("<Esc>").map(String::as_str),
            Some("visual_exit")
        );
        // `v` acts on the moving end, `o` on the other one.
        assert_eq!(
            config.visual_keys.get("vw").map(String::as_str),
            Some("visual_scope_word")
        );
        assert_eq!(
            config.visual_keys.get("ow").map(String::as_str),
            Some("visual_other_word")
        );
        assert_eq!(
            config.visual_keys.get("o").map(String::as_str),
            Some("visual_swap_ends")
        );
        assert_eq!(
            config.visual_keys.get("oo").map(String::as_str),
            Some("visual_swap_ends")
        );
        // User override is merged in.
        assert_eq!(
            config.visual_keys.get("y").map(String::as_str),
            Some("visual_exit")
        );
        // The enter bindings live in the normal table.
        assert_eq!(
            config.keys.get("v").map(String::as_str),
            Some("visual_enter")
        );
        assert_eq!(
            config.keys.get("vp").map(String::as_str),
            Some("visual_enter_paragraph")
        );
    }

    #[test]
    fn pre_collapse_focus_tables_get_a_migration_hint() {
        let err = Config::from_toml(
            r#"
            [word_focus_keys]
            "x" = "word_focus_right"
            "#,
        )
        .unwrap_err();
        // The underlying serde message survives...
        assert!(err.contains("word_focus_keys"), "{err}");
        // ...and is followed by something actionable.
        assert!(err.contains("[focus_keys]"), "{err}");
        assert!(err.contains("focus_left"), "{err}");
        // The example it cites must be a command name that really existed --
        // a migration hint naming a command nobody ever had is worse than none.
        assert!(err.contains("word_focus_left"), "{err}");
    }

    #[test]
    fn unrelated_parse_errors_get_no_migration_hint() {
        let err = Config::from_toml("[view]\nscroll_step = \"not a number\"\n").unwrap_err();
        assert!(!err.contains("hint:"), "{err}");
    }

    #[test]
    fn parses_files_section() {
        let config = Config::from_toml(
            r#"
            [files]
            open_dir = "/some/path"
            "#,
        )
        .unwrap();
        assert_eq!(config.files.open_dir.as_deref(), Some("/some/path"));
        // Default has no override.
        assert_eq!(Config::default().files.open_dir, None);
    }

    #[test]
    fn unknown_field_is_a_useful_error() {
        let err = Config::from_toml("[view]\nscrol_step = 10.0\n").unwrap_err();
        assert!(
            err.contains("scrol_step"),
            "error should name the field: {err}"
        );
    }

    #[test]
    fn type_mismatch_is_an_error() {
        let err = Config::from_toml("[view]\nscroll_step = \"fast\"\n").unwrap_err();
        assert!(err.contains("scroll_step") || err.contains("invalid type"));
    }

    #[test]
    fn missing_file_yields_defaults() {
        let config = Config::load(Path::new("/nonexistent/syodep/config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn load_reads_file_and_reports_parse_errors_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not valid toml [[").unwrap();
        let err = Config::load(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("config.toml"), "{msg}");
    }

    #[test]
    fn default_config_doc_round_trips_to_defaults() {
        let doc = default_config_doc();
        let parsed = Config::from_toml(&doc)
            .unwrap_or_else(|e| panic!("generated doc must parse: {e}\n---\n{doc}"));
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn default_config_doc_lists_every_option_and_table() {
        let doc = default_config_doc();
        // Every [view] field name must appear, so a newly added field can't be
        // silently dropped from the generated template.
        let value = toml::Value::try_from(Config::default()).unwrap();
        let table = value.as_table().unwrap();
        let view = table["view"].as_table().unwrap();
        for field in view.keys() {
            assert!(
                doc.contains(field),
                "[view] field missing from doc: {field}"
            );
        }
        // Every top-level section/table header must appear.
        for section in table.keys() {
            assert!(
                doc.contains(&format!("[{section}]")),
                "section header missing from doc: [{section}]"
            );
        }
    }

    #[test]
    fn default_config_doc_emits_active_values() {
        let doc = default_config_doc();
        assert!(doc.contains("scroll_step = 60.0"), "{doc}");
        assert!(doc.contains("\"j\" = \"scroll_down\""), "{doc}");
        // Both overlay tables are emitted, with their scope-dispatching
        // commands rather than per-scope ones.
        assert!(doc.contains("[focus_keys]"), "{doc}");
        assert!(doc.contains("\"h\" = \"focus_left\""), "{doc}");
        assert!(doc.contains("[visual_keys]"), "{doc}");
    }
}
