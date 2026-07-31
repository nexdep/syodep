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
| `scroll_off` | float | `80.0` | pixels kept between the focused text and the top/bottom edge while the view follows it |
| `page_gap` | float | `12.0` | gap between pages, in PDF points (1/72 in at 100% zoom) |
| `default_zoom` | float | `1.0` | zoom for documents without a saved position (used when `fit_width_on_open = false`) |
| `fit_width_on_open` | bool | `true` | fit page width to window when opening a document without a saved position |
| `zoom_step` | float | `1.1` | multiplicative step for `zoom_in`/`zoom_out` |
| `background` | string | `"#1e1e1e"` | canvas background color, `#rrggbb` |
| `focus_color` | string | `"#5b9bd5"` | highlight for focus mode (one colour shared by every scope), `#rrggbb` |
| `focus_opacity` | float | `0.55` | opacity of the focus highlight, `0.0`-`1.0` |
| `visual_color` | string | `"#8a8a8a"` | highlight for the visual-mode selection, `#rrggbb` |
| `visual_opacity` | float | `0.55` | opacity of the selection highlight, `0.0`-`1.0` |
| `highlight_color` | string | `"#ffd400"` | colour of a highlight, `#rrggbb` — also what is written into the PDF on save |
| `highlight_opacity` | float | `0.4` | opacity of the highlight overlay, `0.0`-`1.0` |
| `detect_tables` | bool | `true` | treat each detected table as one stop from line scope up, drawn as one box |
| `detect_headings` | bool | `true` | treat each detected heading as one step at sentence and paragraph scope |
| `detect_equations` | bool | `true` | treat each detected display equation as one stop from line scope up, drawn as one box |
| `skip_page_furniture` | bool | `true` | keep running headers, page numbers and sideways text out of the caret's path |
| `detect_footnotes` | bool | `true` | treat each detected footnote as one stop from line scope up, drawn as one box; `s`/`p` skip it entirely |

Documents with a saved reading position restore their previous scroll and
zoom instead of applying `default_zoom`/`fit_width_on_open`.

**Scroll-off.** In focus, visual and highlight mode the view follows the
highlight as it moves. `scroll_off` is how much canvas it keeps between the
highlight and the top or bottom edge while doing so — Vim's `scrolloff`,
measured in screen pixels rather than lines, since a PDF's line height varies
within and between documents. At the default `80.0` the page starts scrolling
under the highlight about three body lines before it would reach the border,
so there is always text past it to read into. The buffer applies to the
moving end only: in visual mode the anchor may be arbitrarily far off screen.

It is a lower bound on scrolling, never a push: at the very start and end of a
document there is nothing left to scroll to, so the buffer is given up and the
first and last lines stay reachable flush against the edge — as in Vim at the
ends of a buffer. A span taller than the window minus both buffers gets a
proportionally smaller one, so a full-page figure does not oscillate between
the two constraints. Set `scroll_off = 0.0` to let the highlight sit flush
with the edge. Horizontal motion is unaffected.

**Table detection.** With `detect_tables = true` a table is a single unit from
line scope up: one `e` steps onto it, the next steps past it, selecting it in
visual mode takes the whole table, and its highlight is one rectangle over the
whole thing rather than a strip per row. Word and char scope (`cw`, `cc`) still
walk through the words and characters inside, so a single figure in a table
stays selectable. Detection is a heuristic and costs a second text-extraction
pass per page; set it to `false` to navigate tables line by line as before.
Images go further — they are single stops at word scope too, having no words
inside.

**Heading detection.** With `detect_headings = true` a heading is one step at
sentence and paragraph scope: `s` lands on it and the next `s` lands on the body
beneath, and it is never glued to the following text for want of a full stop.
Numbered headings such as `2.12. Recommended checking order` count as a single
sentence despite their periods. A heading goes further than a table — `w` walks
its individual words *and* `j` at line scope still moves line by line — because
its wrapped lines are real reading lines, ordinary prose you may want part of. A line
counts as a heading when it is set noticeably larger than the page's body text,
or when it is entirely bold at body size and does not fill the column width, or
when it opens with a multi-level section number and title (`1.1. Methods`,
`2.12. Recommended checking order`) — the shape rule that catches subsection
headings set at body size. Detection costs nothing extra to extract; set it to
`false` if the heuristic misjudges a document.

**Equation detection.** With `detect_equations = true` a display equation is one
stop from line scope up, exactly like a table. A formula rarely ends in a full
stop, so it would otherwise be glued to the sentence before it — and a stop
*inside* one (`f(x) = 0.`) would split it. Its rows are not reading lines
either, so `j` steps over the whole system rather than row by row, and the
highlight is the formula's whole box rather than a ragged strip per row. `w` and
`h`/`l` still walk through it, which is what keeps a single variable
selectable.

A line counts as a display equation when it is set apart from the prose (it does
not fill the column width), reads as mathematics either by its fonts or by rich
math characters (Greek / unicode operators — ASCII `+`/`=` alone is not enough),
carries an operator or relation, and carries almost no ordinary words. An
aligned system of several lines is one equation, and an equation number set on a
line of its own (`(3.4)`) belongs to the equation beside it.

**Maths written inline in a sentence is deliberately left alone**: to become one
step it would have to be a region, and a region would split the sentence around
it. Detection costs nothing extra to extract; set it to `false` if the heuristic
misjudges a document.

**Page furniture.** With `skip_page_furniture = true` the caret never traverses
a running header, a page number, a manuscript line-numbering column, a
sideways stamp down a margin, or an inclined watermark: they are dropped from
the navigable content layer entirely, at every scope, and no selection can
cover them. They are still drawn on the page, and still extracted — only
navigation ignores them. There is no key to toggle this mid-document by
design; it is a setting.

Three independent rules find them:

- **Repetition** — a margin-band line whose text (with digit runs masked, so
  page numbers and `Chapter 7 of 9` still match themselves) and baseline recur
  across pages. That is why a paper's title, which appears once, is never
  mistaken for a running head. A header and a folio sharing one baseline —
  common in facing-page layouts, where the pair swaps sides between recto and
  verso — are matched independently of which side either one is on or which
  one a page happens to put first. Bare page numbers may sit a little further
  into the page than the strict margin band (journal folios often do) and still
  count.
- **Line numbering** — a run of short, purely numeric lines forming their own
  column at the page's left margin, clearly separated from the body text
  beside them: the manuscript line-numbering of a submission or review draft,
  where every body line is numbered and the count restarts each page. Unlike
  repetition this needs no cross-page evidence — the pattern is visible on a
  single page — and it is never confused with a genuinely numeric page (a
  table of figures), since there is then no body-text left edge to measure the
  gutter against.
- **Rotation** — anything not running in the page's own dominant direction, so
  a page laid out entirely sideways keeps all of it; **rotated column labels
  inside a table are skipped too**, which is the one case where this removes
  something you might have wanted.

Set it to `false` to walk every extracted line as before. Note that `Ln 1` in
the status line then means the page's first *content* line rather than its first
body line.

**Footnote detection.** With `detect_footnotes = true` a footnote is one stop
from line scope up, drawn as one box — the same shape as a table, not a
heading: it is reachable only by deliberately walking into it with word or
char scope (`cw`, `cc`), never by stepping through it line by line. `s` and
`p`, reading through a page's ordinary body prose, additionally skip a
footnote **entirely** rather than costing it a single stop the way a table
does — the goal is that reading through a page never lands on one by
accident. A caret placed inside a footnote deliberately still expands and
steps through its own sentences normally once there; only the automatic
forward/backward search treats it as invisible. Unlike page furniture, a
footnote is never removed from the navigable content — it is real reading
matter, just out of the way of ordinary reading.

A line counts as a footnote when it sits in the page's bottom margin band and
is set noticeably smaller than the page's body text — the mirror of heading
detection's larger-than-body rule. Detection costs nothing extra to extract;
set it to `false` if the heuristic misjudges a document.

**Overlay colours.** The one focus mode's five scopes (char, line, word,
sentence, paragraph) all share `focus_color`: the highlight tells you that
focus is active, not which scope you are in. The selection uses
`visual_color`, and a highlight — pending or stored — uses `highlight_color`.
Overlays are drawn as plain filled boxes with no border, and overlapping boxes
are merged before filling, so a multi-line highlight is one flat block rather
than a ladder of edges with darker seams.

The defaults are mid-tone colours at `0.55` opacity, which over a white page
blend to about `#a5c8e8` (focus) and `#bfbfbf` (selection) — clearly visible at
a glance while leaving the text under them fully legible. The highlight
default is pure highlighter yellow `#ffd400` at `0.4` opacity, which over a
white page blends to `#ffee99`. Lower the opacity for a fainter tint, or raise
it towards `1.0` for a solid block.

`highlight_opacity` also carries over into a saved PDF: a highlight
annotation is always painted with Multiply blending (every reader does this,
not just syodep's own overlay), so the same opacity is written into the
annotation's constant alpha (`/CA`) — otherwise a highlight previewed at less
than full opacity would come out stronger once saved, since Multiply blending
alone does not fade a colour, only opacity does. The live overlay previews
this faithfully by compositing with the same blend before drawing, rather
than the plain alpha blend `focus_color`/`visual_color` use.

An unparseable colour falls back to its default and reports the problem in the
status line rather than leaving the overlay invisible. Only `#rrggbb` is
accepted — opacity is a separate option, so an eight-digit value is rejected
rather than silently interpreted.

## `[files]`

| Option | Type | Default | Meaning |
|---|---|---|---|
| `open_dir` | string | *(unset)* | starting directory for the Open dialog (the `open_file` command, `<leader>o`) |

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
| `leader` | string | `"<Space>"` | the key sequence `<leader>` stands for in bindings |

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

**The leader.** `leader` is the sequence `<leader>` expands to in a binding —
`<leader>w` is `save_document` by default, so `<Space>` then `w` saves. Expansion
happens when the config is read, so a leader binding is an ordinary sequence
afterwards and follows the same prefix and pause rules as `gg`. Any sequence
works, though a single otherwise-unbound key is the point. An unparseable value
falls back to `<Space>` with a warning in the status bar, rather than costing you
every `<leader>` binding you have. `<leader>` itself is not accepted here, so a
leader cannot refer to itself.

```toml
[input]
timeout_ms = 500
leader = "<Space>"
```

## `[keys]`

A table of `"key sequence" = "command name"` entries that overlay the
default keybindings (only your changes need to be listed). Key syntax and
the default bindings: `docs/keybindings.md`. Command names:
`docs/commands.md`.

```toml
[keys]
"j"     = "scroll_half_page_down"  # rebind a default
"<C-o>" = "open_file"              # add a new binding, alongside the default <leader>o
```

## `[focus_keys]`

Keybindings that apply only in **focus mode** (entered with `cc`, `cw`, `ce`,
`cs` or `cp`). They overlay the normal `[keys]` while focus mode is active, so
`hjkl`/`<Esc>` can mean focus motions there while every other binding keeps its
normal behavior. Like `[keys]`, entries overlay the defaults — list only
changes.

Defaults: `h`/`j`/`k`/`l` (and the arrow keys) move the highlight by one unit
of the active scope, `w`/`b` move by word runs whatever the scope, `e` by
lines, and `<Esc>` exits.

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

## `[highlight_keys]`

Keybindings that apply only while a **highlight** is pending (entered with `a`
from focus or visual mode). They overlay the normal `[keys]`.

The defaults are the visual-mode motions bound to *the same commands* —
`hjkl`/arrows, `w`/`b`/`e`/`s`/`p`, `o`, `oo` and `oc`/`oe`/`ow`/`os`/`op` — since
a pending highlight is a selection and reshaping it must not be a second
implementation of reshaping a selection. Only three keys are specific to it: `a`
(`highlight_commit`) keeps the highlight, and `<Esc>` or `<BS>`
(`highlight_discard`) throws it away.

`v` and `c` are deliberately absent, so they fall through to `[keys]` and store
the highlight on the way into the mode they name. See `docs/keybindings.md` and
`docs/commands-highlight-mode.md`.

```toml
[highlight_keys]
"y" = "highlight_commit"       # extra binding, only while highlighting
```

## Planned config sections

Later phases add: annotation
preferences (default highlight color etc.), and external commands. They
will be documented here as they land (see `docs/roadmap.md`).
