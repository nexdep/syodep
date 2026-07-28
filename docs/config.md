# Configuration

syodep reads a single human-editable TOML file:

| Platform | Path |
|---|---|
| Linux | `$XDG_CONFIG_HOME/syodep/config.toml` (default `~/.config/syodep/config.toml`) |
| Windows | `%APPDATA%\syodep\config.toml` |

A fully commented sample lives at `config/default-config.toml` in the
repository. Every value is optional; omitted values use built-in defaults.

Run `syodep --defaults` to write a `syodep_defaults.config.toml` into the
current directory: a complete, always-current template with every option set
to its built-in default (generated from the running build, so it never drifts).
Copy it to the config path above to use it.

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
| `focus_color` | string | `"#add8e6"` | highlight for every focus mode, `#rrggbb` |
| `focus_opacity` | float | `0.4` | opacity of the focus highlight, `0.0`-`1.0` |
| `visual_color` | string | `"#d3d3d3"` | highlight for the visual-mode selection, `#rrggbb` |
| `visual_opacity` | float | `0.4` | opacity of the selection highlight, `0.0`-`1.0` |

Documents with a saved reading position restore their previous scroll and
zoom instead of applying `default_zoom`/`fit_width_on_open`.

**Overlay colours.** All five focus modes (caret, line, word, sentence,
paragraph) share `focus_color`: the highlight tells you that focus is active,
not which scope you are in. The selection uses `visual_color`. Overlays are
drawn as plain filled boxes with no border, and overlapping boxes are merged
before filling, so a multi-line highlight is one flat block rather than a
ladder of edges with darker seams.

An unparseable colour falls back to its default and reports the problem in the
status line rather than leaving the overlay invisible. Only `#rrggbb` is
accepted — opacity is a separate option, so an eight-digit value is rejected
rather than silently interpreted.

## `[files]`

| Option | Type | Default | Meaning |
|---|---|---|---|
| `open_dir` | string | *(unset)* | starting directory for the Open dialog (the `open_file` command, `<C-o>`) |

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

## `[input]`

| Option | Type | Default | Meaning |
|---|---|---|---|
| `timeout_ms` | integer (ms) | `500` | how long a half-typed key sequence waits before acting on its own; `0` disables the pause |

Some keys are both a command and the start of a longer sequence — `c`, `v`, and
`o` while selecting. Rather than firing eagerly, syodep waits to see whether
another key follows. The pause is the second way that wait can end: press the
key, stop, and it acts.

Sequences typed at normal speed never reach it, so `cw` still means word focus.
Raise it if you type chords slowly and find them splitting in two; lower it if a
deliberate pause feels sluggish. `0` restores the original behaviour, where only
the next key press ever ends a wait — with the consequence that a binding which
is also a prefix can then only be reached by following it with an unrelated key.

See `docs/keybindings.md` for the full disambiguation rule.

```toml
[input]
timeout_ms = 500
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

## `[focus_keys]`

Keybindings that apply only in **focus mode** (entered with `cc`, `cw`, `ce`,
`cs` or `cp`). They overlay the normal `[keys]` while focus mode is active, so
`hjkl`/`<Esc>` can mean focus motions there while every other binding keeps its
normal behavior. Like `[keys]`, entries overlay the defaults — list only
changes.

Defaults: `h`/`j`/`k`/`l` (and the arrow keys) move the highlight by one unit
of the active scope, `w`/`e`/`b` move by word runs whatever the scope, and
`<Esc>` exits.

**One table covers all five scopes.** The motion commands dispatch on the
active scope — `focus_left` is a character in char scope, a word in word scope,
a column jump in line scope and the previous unit in sentence or paragraph
scope — so there is nothing scope-specific left to bind. See
`docs/keybindings.md` for the full description and `docs/commands-focus-mode.md`
for the `focus_*` command names.

```toml
[focus_keys]
"x" = "focus_right"   # extra binding, only in focus mode
```

## `[visual_keys]`

Keybindings that apply only in **visual mode** (entered with `v`, or with
`vc`/`ve`/`vw`/`vs`/`vp` to name the granularity). They overlay the normal
`[keys]` while visual mode is active. Defaults: `hjkl` and the arrow keys move
the active end of the selection by one unit of its scope; `w`/`b`/`e` move it
by a word whatever the scope; `o` switches which end moves (`oo` does so
without waiting for a motion) and `oc`/`oe`/`ow`/`os`/`op` switch ends *and*
set that end's scope; `vc`/`ve`/`vw`/`vs`/`vp` set the active end's scope
without switching; `<Esc>` (or `v`) exits. See `docs/keybindings.md` for the
full description and `docs/commands-visual-mode.md` for the `visual_*` command
names.

```toml
[visual_keys]
"y" = "visual_exit"            # extra binding, only in visual mode
```

## Planned config sections

Later phases add: annotation
preferences (default highlight color etc.), and external commands. They
will be documented here as they land (see `docs/roadmap.md`).
