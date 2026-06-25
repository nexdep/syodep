# Configuration

syodep reads a single human-editable TOML file:

| Platform | Path |
|---|---|
| Linux | `$XDG_CONFIG_HOME/syodep/config.toml` (default `~/.config/syodep/config.toml`) |
| Windows | `%APPDATA%\syodep\config.toml` |

A fully commented sample lives at `config/default-config.toml` in the
repository. Every value is optional; omitted values use built-in defaults.

**Error handling:** an unreadable or invalid config never prevents syodep
from starting. The parse error (with the offending field) is shown in the
status bar and built-in defaults are used. Unknown fields are rejected (to
catch typos like `scrol_step`), with the field named in the error.

**What does NOT go here:** dynamic user state — reading positions,
bookmarks, highlights, notes, history. That lives in the SQLite database
(`~/.local/share/syodep/syodep.sqlite3` on Linux,
`%APPDATA%\syodep\syodep.sqlite3` on Windows).

## `[view]`

| Option | Type | Default | Meaning |
|---|---|---|---|
| `scroll_step` | float | `60.0` | vertical pixels per `scroll_down`/`scroll_up` step |
| `horizontal_scroll_step` | float | `60.0` | horizontal pixels per `scroll_left`/`scroll_right` step |
| `page_gap` | float | `12.0` | gap between pages, in PDF points (1/72 in at 100% zoom) |
| `default_zoom` | float | `1.0` | zoom for documents without a saved position (used when `fit_width_on_open = false`) |
| `fit_width_on_open` | bool | `true` | fit page width to window when opening a document without a saved position |
| `zoom_step` | float | `1.1` | multiplicative step for `zoom_in`/`zoom_out` |
| `background` | string | `"#1e1e1e"` | canvas background color, `#rrggbb` |

Documents with a saved reading position restore their previous scroll and
zoom instead of applying `default_zoom`/`fit_width_on_open`.

## `[keys]`

A table of `"key sequence" = "command name"` entries that overlay the
default keybindings (only your changes need to be listed). Key syntax and
the default bindings: `docs/keybindings.md`. Command names:
`docs/commands.md`.

```toml
[keys]
"j"     = "scroll_half_page_down"
"<C-o>" = "open_file"
```

## `[caret_focus_keys]`

Keybindings that apply only in **caret focus mode** (entered with `cc`). They
overlay the normal `[keys]` while caret focus mode is active, so `hjkl`/`<Esc>`
can mean caret motions there while every other binding keeps its normal
behavior. Like `[keys]`, entries overlay the defaults — list only changes.
Defaults: `h`/`j`/`k`/`l` (and the arrow keys) move the caret,
`w`/`e`/`b` move by word runs, and `<Esc>` exits. See
`docs/keybindings.md` for the full description and `docs/commands.md` for
the `caret_focus_*` command names.

```toml
[caret_focus_keys]
"x" = "caret_focus_right"   # extra binding, only in caret focus mode
```

## Planned config sections

Later phases add: theme/colors beyond the background, annotation
preferences (default highlight color etc.), and external commands. They
will be documented here as they land (see `docs/roadmap.md`).
