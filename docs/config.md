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
| `detect_tables` | bool | `true` | treat each detected table as one stop for focus and selection motions |
| `detect_headings` | bool | `true` | treat each detected heading as one step at sentence and paragraph scope |
| `skip_page_furniture` | bool | `true` | keep running headers, page numbers and sideways text out of the caret's path |

Documents with a saved reading position restore their previous scroll and
zoom instead of applying `default_zoom`/`fit_width_on_open`.

**Table detection.** With `detect_tables = true` a table is a single unit for
every motion above char scope, the way an image already is: one `w` steps onto
it, the next steps past it, and selecting it in visual mode takes the whole
table. Char scope (`cc`) still walks through the characters inside, so a single
figure in a table stays selectable. Detection is a heuristic and costs a second
text-extraction pass per page; set it to `false` to navigate tables line by line
as before. Images are unaffected — they are always single stops.

**Heading detection.** With `detect_headings = true` a heading is one step at
sentence and paragraph scope: `s` lands on it and the next `s` lands on the body
beneath, and it is never glued to the following text for want of a full stop.
Numbered headings such as `2.12. Recommended checking order` count as a single
sentence despite their periods. Unlike a table a heading is *not* atomic — `w`
still walks its individual words and `j` at line scope still moves line by line
— because a heading is ordinary prose you may want to select part of. A line
counts as a heading when it is set noticeably larger than the page's body text,
or when it is entirely bold at body size and does not fill the column width.
Detection costs nothing extra to extract; set it to `false` if the heuristic
misjudges a document.

**Page furniture.** With `skip_page_furniture = true` the caret never traverses
a running header, a page number, a sideways stamp down a margin, or an inclined
watermark: they are dropped from the navigable content layer entirely, at every
scope, and no selection can cover them. They are still drawn on the page, and
still extracted — only navigation ignores them. There is no key to toggle this
mid-document by design; it is a setting.

Headers and footers are recognised by **repeating across pages**, never by
position alone. A line counts only when the same text — with digit runs masked,
so page numbers and `Chapter 7 of 9` still match themselves — appears at the
same height on other pages. That is why a paper's title, which appears once, is
never mistaken for a running head. Sideways text is anything not running in the
page's own dominant direction, so a page laid out entirely sideways keeps all of
it; **rotated column labels inside a table are skipped too**, which is the one
case where this removes something you might have wanted.

Set it to `false` to walk every extracted line as before. Note that `Ln 1` in
the status line then means the page's first *content* line rather than its first
body line.

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
