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

## `[files]`

| Option | Type | Default | Meaning |
|---|---|---|---|
| `open_dir` | string | *(unset)* | starting directory for the Open dialog (the `open_file`/`o` command) |

When `open_dir` is unset, the Open dialog starts in the directory syodep was
launched from (the process working directory) — useful when launching from a
terminal inside a paper or project folder. When set, it must be an absolute
path (`~` is not expanded). If the configured path does not exist or is not a
directory, syodep falls back to the launch directory and shows a warning.

`syodep --check` reports the resolved directory and where it came from, under
*Configuration → Open dialog dir*.

```toml
[files]
open_dir = "/home/me/papers"
```

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

## `[line_focus_keys]`

Keybindings that apply only in **line focus mode** (entered with `cl`). They
overlay the normal `[keys]` while line focus mode is active, mirroring
`[caret_focus_keys]`. Defaults: `j`/`k` (and `<Up>`/`<Down>`) move the
highlight line-wise, `h`/`l` (and `<Left>`/`<Right>`) move between columns,
`<Esc>` exits. See `docs/keybindings.md` for the full description and
`docs/commands.md` for the `line_focus_*` command names.

```toml
[line_focus_keys]
"w" = "line_focus_right"   # extra binding, only in line focus mode
```

## `[word_focus_keys]`

Keybindings that apply only in **word focus mode** (entered with `cw`). They
overlay the normal `[keys]` while word focus mode is active, mirroring the
other focus-mode key tables. Defaults: `h`/`b` (and `<Left>`) move to the
previous word run, `l`/`w` (and `<Right>`) move to the next word run, `j`/`k`
(and `<Down>`/`<Up>`) move line-wise, and `<Esc>` exits. See
`docs/keybindings.md` for the full description and `docs/commands.md` for
the `word_focus_*` command names.

```toml
[word_focus_keys]
"e" = "word_focus_right"   # extra binding, only in word focus mode
```

## `[sentence_focus_keys]`

Keybindings that apply only in **sentence focus mode** (entered with `cs`). They
overlay the normal `[keys]` while sentence focus mode is active. Defaults:
`h`/`k` (and `<Left>`/`<Up>`) move to the previous sentence, `l`/`j` (and
`<Right>`/`<Down>`) move to the next, and `<Esc>` exits. See
`docs/keybindings.md` for the full description and `docs/commands.md` for the
`sentence_focus_*` command names.

```toml
[sentence_focus_keys]
"n" = "sentence_focus_next"   # extra binding, only in sentence focus mode
```

## `[paragraph_focus_keys]`

Keybindings that apply only in **paragraph focus mode** (entered with `cp`). They
overlay the normal `[keys]` while paragraph focus mode is active. Defaults:
`h`/`k` (and `<Left>`/`<Up>`) move to the previous paragraph, `l`/`j` (and
`<Right>`/`<Down>`) move to the next, and `<Esc>` exits. See
`docs/keybindings.md` for the full description and `docs/commands.md` for the
`paragraph_focus_*` command names.

```toml
[paragraph_focus_keys]
"n" = "paragraph_focus_next"   # extra binding, only in paragraph focus mode
```

## Planned config sections

Later phases add: theme/colors beyond the background, annotation
preferences (default highlight color etc.), and external commands. They
will be documented here as they land (see `docs/roadmap.md`).
