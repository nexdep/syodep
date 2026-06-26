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
    /// Caret-focus-mode keybindings (the `[caret_focus_keys]` table). These overlay
    /// the normal `keys` while caret focus mode is active, so `hjkl`/`<Esc>` can mean
    /// something different there while every other binding still works.
    #[serde(default)]
    pub caret_focus_keys: BTreeMap<String, String>,
    /// Line-focus-mode keybindings (the `[line_focus_keys]` table). These overlay
    /// the normal `keys` while line focus mode is active, mirroring
    /// `caret_focus_keys`.
    #[serde(default)]
    pub line_focus_keys: BTreeMap<String, String>,
    /// Word-focus-mode keybindings (the `[word_focus_keys]` table). These overlay
    /// the normal `keys` while word focus mode is active, mirroring
    /// `caret_focus_keys` and `line_focus_keys`.
    #[serde(default)]
    pub word_focus_keys: BTreeMap<String, String>,
    /// Sentence-focus-mode keybindings (the `[sentence_focus_keys]` table). These
    /// overlay the normal `keys` while sentence focus mode is active.
    #[serde(default)]
    pub sentence_focus_keys: BTreeMap<String, String>,
    /// Paragraph-focus-mode keybindings (the `[paragraph_focus_keys]` table).
    /// These overlay the normal `keys` while paragraph focus mode is active.
    #[serde(default)]
    pub paragraph_focus_keys: BTreeMap<String, String>,
    /// `[files]` section: file-dialog and path behaviour.
    #[serde(default)]
    pub files: FilesConfig,
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
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            view: ViewConfig::default(),
            keys: default_keybindings(),
            caret_focus_keys: default_caret_focus_keybindings(),
            line_focus_keys: default_line_focus_keybindings(),
            word_focus_keys: default_word_focus_keybindings(),
            sentence_focus_keys: default_sentence_focus_keybindings(),
            paragraph_focus_keys: default_paragraph_focus_keybindings(),
            files: FilesConfig::default(),
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
        ("cc", "caret_focus_enter"),
        ("cl", "line_focus_enter"),
        ("cw", "word_focus_enter"),
        ("cs", "sentence_focus_enter"),
        ("cp", "paragraph_focus_enter"),
        ("o", "open_file"),
        ("q", "quit"),
        ("<Esc>", "cancel"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Built-in caret-focus-mode keybindings (the `[caret_focus_keys]` table). These overlay
/// the normal bindings while caret focus mode is active: `hjkl` move the caret and
/// `<Esc>` leaves caret focus mode, while everything else keeps its normal meaning.
///
/// Every entry here must be documented in `docs/keybindings.md`.
pub fn default_caret_focus_keybindings() -> BTreeMap<String, String> {
    [
        ("h", "caret_focus_left"),
        ("j", "caret_focus_down"),
        ("k", "caret_focus_up"),
        ("l", "caret_focus_right"),
        ("w", "caret_focus_next_word"),
        ("e", "caret_focus_end_word"),
        ("b", "caret_focus_prev_word"),
        ("<Left>", "caret_focus_left"),
        ("<Down>", "caret_focus_down"),
        ("<Up>", "caret_focus_up"),
        ("<Right>", "caret_focus_right"),
        ("<Esc>", "caret_focus_exit"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Built-in line-focus-mode keybindings (the `[line_focus_keys]` table). These
/// overlay the normal bindings while line focus mode is active: `j`/`k` move the
/// highlight line-wise, `h`/`l` move between columns, and `<Esc>` leaves the mode,
/// while everything else keeps its normal meaning.
///
/// Every entry here must be documented in `docs/keybindings.md`.
pub fn default_line_focus_keybindings() -> BTreeMap<String, String> {
    [
        ("h", "line_focus_left"),
        ("j", "line_focus_down"),
        ("k", "line_focus_up"),
        ("l", "line_focus_right"),
        ("<Left>", "line_focus_left"),
        ("<Down>", "line_focus_down"),
        ("<Up>", "line_focus_up"),
        ("<Right>", "line_focus_right"),
        ("<Esc>", "line_focus_exit"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Built-in word-focus-mode keybindings (the `[word_focus_keys]` table). These
/// overlay the normal bindings while word focus mode is active: `h`/`l` (and
/// `w`/`b`) step word-wise, `j`/`k` move by line, and `<Esc>` leaves the mode,
/// while everything else keeps its normal meaning.
///
/// Every entry here must be documented in `docs/keybindings.md`.
pub fn default_word_focus_keybindings() -> BTreeMap<String, String> {
    [
        ("h", "word_focus_left"),
        ("j", "word_focus_down"),
        ("k", "word_focus_up"),
        ("l", "word_focus_right"),
        ("w", "word_focus_right"),
        ("b", "word_focus_left"),
        ("<Left>", "word_focus_left"),
        ("<Down>", "word_focus_down"),
        ("<Up>", "word_focus_up"),
        ("<Right>", "word_focus_right"),
        ("<Esc>", "word_focus_exit"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Built-in sentence-focus-mode keybindings (the `[sentence_focus_keys]` table).
/// These overlay the normal bindings while sentence focus mode is active.
/// Sentences are a linear sequence, so all of `hjkl`/arrows collapse to
/// previous/next and `<Esc>` leaves the mode; everything else keeps its meaning.
///
/// Every entry here must be documented in `docs/keybindings.md`.
pub fn default_sentence_focus_keybindings() -> BTreeMap<String, String> {
    [
        ("h", "sentence_focus_prev"),
        ("k", "sentence_focus_prev"),
        ("<Up>", "sentence_focus_prev"),
        ("<Left>", "sentence_focus_prev"),
        ("l", "sentence_focus_next"),
        ("j", "sentence_focus_next"),
        ("<Down>", "sentence_focus_next"),
        ("<Right>", "sentence_focus_next"),
        ("<Esc>", "sentence_focus_exit"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Built-in paragraph-focus-mode keybindings (the `[paragraph_focus_keys]`
/// table). These overlay the normal bindings while paragraph focus mode is
/// active, mirroring `sentence_focus_keys`: `hjkl`/arrows collapse to
/// previous/next and `<Esc>` leaves the mode.
///
/// Every entry here must be documented in `docs/keybindings.md`.
pub fn default_paragraph_focus_keybindings() -> BTreeMap<String, String> {
    [
        ("h", "paragraph_focus_prev"),
        ("k", "paragraph_focus_prev"),
        ("<Up>", "paragraph_focus_prev"),
        ("<Left>", "paragraph_focus_prev"),
        ("l", "paragraph_focus_next"),
        ("j", "paragraph_focus_next"),
        ("<Down>", "paragraph_focus_next"),
        ("<Right>", "paragraph_focus_next"),
        ("<Esc>", "paragraph_focus_exit"),
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

impl Config {
    /// Parse a configuration from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let mut config: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        // An empty or missing [keys] table means "use the defaults". Users who
        // want extra bindings list only their additions; defaults still apply
        // unless explicitly rebound.
        let mut keys = default_keybindings();
        keys.extend(std::mem::take(&mut config.keys));
        config.keys = keys;
        let mut caret_focus_keys = default_caret_focus_keybindings();
        caret_focus_keys.extend(std::mem::take(&mut config.caret_focus_keys));
        config.caret_focus_keys = caret_focus_keys;
        let mut line_focus_keys = default_line_focus_keybindings();
        line_focus_keys.extend(std::mem::take(&mut config.line_focus_keys));
        config.line_focus_keys = line_focus_keys;
        let mut word_focus_keys = default_word_focus_keybindings();
        word_focus_keys.extend(std::mem::take(&mut config.word_focus_keys));
        config.word_focus_keys = word_focus_keys;
        let mut sentence_focus_keys = default_sentence_focus_keybindings();
        sentence_focus_keys.extend(std::mem::take(&mut config.sentence_focus_keys));
        config.sentence_focus_keys = sentence_focus_keys;
        let mut paragraph_focus_keys = default_paragraph_focus_keybindings();
        paragraph_focus_keys.extend(std::mem::take(&mut config.paragraph_focus_keys));
        config.paragraph_focus_keys = paragraph_focus_keys;
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
    out.push('\n');

    out.push_str(
        "[files]\n\
         # Starting directory for the Open dialog (the \"o\" command). When unset, the\n\
         # dialog opens in the directory syodep was launched from. If the path below\n\
         # does not exist (or is not a directory), syodep falls back to the launch\n\
         # directory. Run `syodep --check` to see which directory is in effect.\n\
         # Use an absolute path (\"~\" is not expanded). Unset by default:\n\
         # open_dir = \"/home/me/papers\"\n\n",
    );

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
        "\n# Caret-focus-mode keybindings (active after pressing \"cc\"). These overlay the\n\
         # normal [keys] while caret focus mode is active: h/j/k/l move the caret\n\
         # (h/l by character, j/k by line) and <Esc> exits.\n",
    );
    push_keytable(
        &mut out,
        "caret_focus_keys",
        &default_caret_focus_keybindings(),
    );

    out.push_str(
        "\n# Line-focus-mode keybindings (active after pressing \"cl\"). j/k move the\n\
         # highlighted line, h/l move between columns and <Esc> exits.\n",
    );
    push_keytable(
        &mut out,
        "line_focus_keys",
        &default_line_focus_keybindings(),
    );

    out.push_str(
        "\n# Word-focus-mode keybindings (active after pressing \"cw\"). h/b move to the\n\
         # previous word run, l/w move to the next, j/k move by line and <Esc> exits.\n",
    );
    push_keytable(
        &mut out,
        "word_focus_keys",
        &default_word_focus_keybindings(),
    );

    out.push_str(
        "\n# Sentence-focus-mode keybindings (active after pressing \"cs\"). Sentences are\n\
         # linear, so hjkl/arrows collapse to previous/next and <Esc> exits.\n",
    );
    push_keytable(
        &mut out,
        "sentence_focus_keys",
        &default_sentence_focus_keybindings(),
    );

    out.push_str(
        "\n# Paragraph-focus-mode keybindings (active after pressing \"cp\"). Like sentence\n\
         # focus, hjkl/arrows collapse to previous/next and <Esc> exits.\n",
    );
    push_keytable(
        &mut out,
        "paragraph_focus_keys",
        &default_paragraph_focus_keybindings(),
    );

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
    fn caret_focus_keys_default_and_user_override() {
        let config = Config::from_toml(
            r#"
            [caret_focus_keys]
            "w" = "caret_focus_right"
            "#,
        )
        .unwrap();
        // Built-in caret bindings survive.
        assert_eq!(
            config.caret_focus_keys.get("h").map(String::as_str),
            Some("caret_focus_left")
        );
        assert_eq!(
            config.caret_focus_keys.get("j").map(String::as_str),
            Some("caret_focus_down")
        );
        assert_eq!(
            config.caret_focus_keys.get("e").map(String::as_str),
            Some("caret_focus_end_word")
        );
        assert_eq!(
            config.caret_focus_keys.get("b").map(String::as_str),
            Some("caret_focus_prev_word")
        );
        // User override is merged in.
        assert_eq!(
            config.caret_focus_keys.get("w").map(String::as_str),
            Some("caret_focus_right")
        );
        // The enter binding (`cc`) lives in the normal table.
        assert_eq!(
            config.keys.get("cc").map(String::as_str),
            Some("caret_focus_enter")
        );
    }

    #[test]
    fn word_focus_keys_default_and_user_override() {
        let config = Config::from_toml(
            r#"
            [word_focus_keys]
            "w" = "word_focus_down"
            "#,
        )
        .unwrap();
        // Built-in word-focus bindings survive.
        assert_eq!(
            config.word_focus_keys.get("h").map(String::as_str),
            Some("word_focus_left")
        );
        assert_eq!(
            config.word_focus_keys.get("l").map(String::as_str),
            Some("word_focus_right")
        );
        assert_eq!(
            config.word_focus_keys.get("j").map(String::as_str),
            Some("word_focus_down")
        );
        assert_eq!(
            config.word_focus_keys.get("<Esc>").map(String::as_str),
            Some("word_focus_exit")
        );
        // User override is merged in.
        assert_eq!(
            config.word_focus_keys.get("w").map(String::as_str),
            Some("word_focus_down")
        );
        // The enter binding (`cw`) lives in the normal table.
        assert_eq!(
            config.keys.get("cw").map(String::as_str),
            Some("word_focus_enter")
        );
    }

    #[test]
    fn sentence_focus_keys_default_and_user_override() {
        let config = Config::from_toml(
            r#"
            [sentence_focus_keys]
            "n" = "sentence_focus_next"
            "#,
        )
        .unwrap();
        // Built-in sentence-focus bindings survive.
        assert_eq!(
            config.sentence_focus_keys.get("h").map(String::as_str),
            Some("sentence_focus_prev")
        );
        assert_eq!(
            config.sentence_focus_keys.get("l").map(String::as_str),
            Some("sentence_focus_next")
        );
        assert_eq!(
            config.sentence_focus_keys.get("<Esc>").map(String::as_str),
            Some("sentence_focus_exit")
        );
        // User override is merged in.
        assert_eq!(
            config.sentence_focus_keys.get("n").map(String::as_str),
            Some("sentence_focus_next")
        );
        // The enter binding (`cs`) lives in the normal table.
        assert_eq!(
            config.keys.get("cs").map(String::as_str),
            Some("sentence_focus_enter")
        );
    }

    #[test]
    fn paragraph_focus_keys_default_and_user_override() {
        let config = Config::from_toml(
            r#"
            [paragraph_focus_keys]
            "n" = "paragraph_focus_next"
            "#,
        )
        .unwrap();
        // Built-in paragraph-focus bindings survive.
        assert_eq!(
            config.paragraph_focus_keys.get("h").map(String::as_str),
            Some("paragraph_focus_prev")
        );
        assert_eq!(
            config.paragraph_focus_keys.get("j").map(String::as_str),
            Some("paragraph_focus_next")
        );
        assert_eq!(
            config.paragraph_focus_keys.get("<Esc>").map(String::as_str),
            Some("paragraph_focus_exit")
        );
        // User override is merged in.
        assert_eq!(
            config.paragraph_focus_keys.get("n").map(String::as_str),
            Some("paragraph_focus_next")
        );
        // The enter binding (`cp`) lives in the normal table.
        assert_eq!(
            config.keys.get("cp").map(String::as_str),
            Some("paragraph_focus_enter")
        );
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
        // The section the stale reference file is missing.
        assert!(doc.contains("[paragraph_focus_keys]"), "{doc}");
    }
}
