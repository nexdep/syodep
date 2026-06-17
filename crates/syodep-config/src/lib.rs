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
use std::path::Path;

use serde::Deserialize;

/// Top-level configuration, mirroring the TOML file.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub view: ViewConfig,
    /// Raw keybindings: key-sequence string -> command-name string.
    /// Parsed and validated into a keymap by `syodep-core`.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// Caret-mode keybindings (the `[caret_keys]` table). These overlay the
    /// normal `keys` while caret mode is active, so `hjkl`/`<Esc>` can mean
    /// something different there while every other binding still works.
    #[serde(default)]
    pub caret_keys: BTreeMap<String, String>,
}

/// `[view]` section: rendering and navigation tunables.
#[derive(Debug, Clone, Deserialize, PartialEq)]
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
            caret_keys: default_caret_keybindings(),
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
        ("c", "caret_enter"),
        ("o", "open_file"),
        ("q", "quit"),
        ("<Esc>", "cancel"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Built-in caret-mode keybindings (the `[caret_keys]` table). These overlay
/// the normal bindings while caret mode is active: `hjkl` move the caret and
/// `<Esc>` leaves caret mode, while everything else keeps its normal meaning.
///
/// Every entry here must be documented in `docs/keybindings.md`.
pub fn default_caret_keybindings() -> BTreeMap<String, String> {
    [
        ("h", "caret_left"),
        ("j", "caret_down"),
        ("k", "caret_up"),
        ("l", "caret_right"),
        ("<Left>", "caret_left"),
        ("<Down>", "caret_down"),
        ("<Up>", "caret_up"),
        ("<Right>", "caret_right"),
        ("<Esc>", "caret_exit"),
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
        let mut caret_keys = default_caret_keybindings();
        caret_keys.extend(std::mem::take(&mut config.caret_keys));
        config.caret_keys = caret_keys;
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
    fn caret_keys_default_and_user_override() {
        let config = Config::from_toml(
            r#"
            [caret_keys]
            "w" = "caret_right"
            "#,
        )
        .unwrap();
        // Built-in caret bindings survive.
        assert_eq!(
            config.caret_keys.get("h").map(String::as_str),
            Some("caret_left")
        );
        assert_eq!(
            config.caret_keys.get("j").map(String::as_str),
            Some("caret_down")
        );
        // User addition is merged in.
        assert_eq!(
            config.caret_keys.get("w").map(String::as_str),
            Some("caret_right")
        );
        // The enter binding lives in the normal table.
        assert_eq!(
            config.keys.get("c").map(String::as_str),
            Some("caret_enter")
        );
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
}
