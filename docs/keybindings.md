# Keybindings

## Key syntax

Bindings use a Vim-flavored textual syntax (shared between the config file
and the internals):

- Plain printable characters bind themselves: `j`, `G`, `+`. Case matters —
  `G` means shift+g and is written as the uppercase character.
- Special keys use angle brackets: `<Esc>`, `<CR>` (Enter), `<Tab>`,
  `<Space>`, `<BS>` (Backspace), `<Up>`, `<Down>`, `<Left>`, `<Right>`,
  `<PageUp>`, `<PageDown>`, `<Home>`, `<End>`.
- Modifiers go inside the brackets: `<C-d>` (ctrl), `<A-x>` (alt),
  `<C-A-Left>` (both). Shift on letters is expressed by case: `<C-G>`.
- A *sequence* concatenates chords: `gg`, `zw`, `g<C-d>`.

**Counts are not part of bindings.** Typing digits before a binding
(`5j`, `12G`) passes a count to the command at runtime. `0` only continues
a count that has already started (so `0` itself is bindable).

**Disambiguation rule:** if a sequence is both a complete binding and a
prefix of a longer one (e.g. binding both `g` and `gg`), syodep waits for
more input rather than firing eagerly; press `<Esc>` to cancel pending
input. There is no timeout — behavior is fully deterministic. The defaults
avoid such overlaps.

## Default bindings

Scrolling:

| Keys | Command |
|---|---|
| `j`, `<Down>` | `scroll_down` |
| `k`, `<Up>` | `scroll_up` |
| `h`, `<Left>` | `scroll_left` |
| `l`, `<Right>` | `scroll_right` |
| `<C-d>` | `scroll_half_page_down` |
| `<C-u>` | `scroll_half_page_up` |
| `<C-f>` | `scroll_page_down` |
| `<C-b>` | `scroll_page_up` |

Page navigation:

| Keys | Command |
|---|---|
| `J`, `<PageDown>` | `next_page` |
| `K`, `<PageUp>` | `prev_page` |
| `gg` | `goto_first_page` (with count: go to that page) |
| `G` | `goto_last_page` (with count: go to that page) |

Zoom:

| Keys | Command |
|---|---|
| `+`, `=` | `zoom_in` |
| `-` | `zoom_out` |
| `zw` | `fit_width` |
| `z0` | `zoom_reset` |

Caret (see "Caret focus mode" below):

| Keys | Command |
|---|---|
| `cc` | `caret_focus_enter` |
| `cl` | `line_focus_enter` |
| `cw` | `word_focus_enter` |
| `cs` | `sentence_focus_enter` |
| `cp` | `paragraph_focus_enter` |

Application:

| Keys | Command |
|---|---|
| `o` | `open_file` |
| `q` | `quit` |
| `<Esc>` | `cancel` |

The mouse wheel (and horizontal trackpad scrolling) also scrolls the view;
this is a convenience, not the primary workflow.

## Caret focus mode

In **normal mode** (the default) `hjkl` scroll the page. Press `cc`
(`caret_focus_enter`) to switch to **caret focus mode**, where a cursor moves
through the document's content — text characters and images:

| Keys | Command |
|---|---|
| `h`, `<Left>` | `caret_focus_left` — one character left |
| `l`, `<Right>` | `caret_focus_right` — one character right |
| `k`, `<Up>` | `caret_focus_up` — one line up (keeps the column) |
| `j`, `<Down>` | `caret_focus_down` — one line down (keeps the column) |
| `w` | `caret_focus_next_word` — next word start |
| `e` | `caret_focus_end_word` — current/next word end |
| `b` | `caret_focus_prev_word` — current/previous word start |
| `<Esc>` | `caret_focus_exit` — back to normal mode |

`h`/`l` step character by character and wrap across lines and pages; `j`/`k`
move line by line, keeping a goal column like a text editor. `w`/`e`/`b`
move by Vim-like word runs: letters/digits/underscore together,
punctuation/symbols separately, whitespace skipped. Each image is a single
caret stop. The view scrolls to keep the caret visible, and counts work
(`5l`, `3j`, `2w`). Every other binding (page scroll, page navigation,
zoom, `q`, `o`, …) still works in caret focus mode — only
`hjkl`/`w`/`e`/`b`/`<Esc>` change meaning. Scroll and page-jump commands
additionally carry the caret to the top of the newly visible content; zoom
leaves it in place. The status bar shows `-- CARET FOCUS --` with the
current line and column. See `docs/commands-caret-focus-mode.md` for the
full list.

Customize caret-focus-mode keys with a `[caret_focus_keys]` table (see
`docs/config.md`); it overlays the normal bindings while caret focus mode is
active. The caret is the foundation for selection, highlighting and search
in later phases (`docs/roadmap.md`).

## Line focus mode

Press `cl` (`line_focus_enter`) to switch to **line focus mode**, where a
whole content line is highlighted:

| Keys | Command |
|---|---|
| `h`, `<Left>` | `line_focus_left` — previous column (multi-column pages) |
| `l`, `<Right>` | `line_focus_right` — next column (multi-column pages) |
| `k`, `<Up>` | `line_focus_up` — one line up |
| `j`, `<Down>` | `line_focus_down` — one line down |
| `<Esc>` | `line_focus_exit` — back to normal mode |

`j`/`k` move the highlight line by line, wrapping across pages; `h`/`l` move
between columns when the page has two or more, keeping the current row (a
no-op on single-column pages). The view scrolls to keep the highlighted line
visible, and counts work (`3j`). As in caret focus mode, every other binding
still works — only `hjkl`/`<Esc>` change meaning — and scroll / page-jump
commands carry the highlight to the top of the newly visible content while
zoom leaves it in place. The status bar shows `-- LINE FOCUS --` with the
current line. See `docs/commands-line-focus-mode.md` for the full list.

Customize line-focus-mode keys with a `[line_focus_keys]` table (see
`docs/config.md`); it overlays the normal bindings while line focus mode is
active.

## Word focus mode

Press `cw` (`word_focus_enter`) to switch to **word focus mode**, where a
whole Vim-like word run is highlighted:

| Keys | Command |
|---|---|
| `h`, `b`, `<Left>` | `word_focus_left` — previous word run |
| `l`, `w`, `<Right>` | `word_focus_right` — next word run |
| `k`, `<Up>` | `word_focus_up` — one line up |
| `j`, `<Down>` | `word_focus_down` — one line down |
| `<Esc>` | `word_focus_exit` — back to normal mode |

Word runs use the same boundaries as caret word motions:
letters/digits/underscore together, punctuation/symbols separately,
whitespace skipped, and each image as a single stop. `j`/`k` move line by
line while keeping a goal column. The view scrolls to keep the highlighted
word visible, and counts work (`3w`, `2j`). As in the other focus modes,
every other binding still works; scroll / page-jump commands carry the
highlight to visible content while zoom leaves it in place. The status bar
shows `-- WORD FOCUS --` with the current line and column. See
`docs/commands-word-focus-mode.md` for the full list.

Customize word-focus-mode keys with a `[word_focus_keys]` table (see
`docs/config.md`); it overlays the normal bindings while word focus mode is
active.

## Sentence focus mode

Press `cs` (`sentence_focus_enter`) to switch to **sentence focus mode**, where
a whole sentence is highlighted — possibly spanning several lines:

| Keys | Command |
|---|---|
| `h`, `k`, `<Left>`, `<Up>` | `sentence_focus_prev` — previous sentence |
| `l`, `j`, `<Right>`, `<Down>` | `sentence_focus_next` — next sentence |
| `<Esc>` | `sentence_focus_exit` — back to normal mode |

A sentence is a run of cells ending at sentence-terminating punctuation
(`.`, `!`, `?`) plus any trailing closing quotes/brackets. Because sentences are
a linear sequence, all of `hjkl` and the arrow keys collapse to previous/next
(there are no Vim `(`/`)` aliases). The highlight spans lines as a
text-selection shape, wraps across pages, and counts work (`3l`). As in the
other focus modes, every other binding still works; scroll / page-jump commands
carry the highlight to visible content while zoom leaves it in place. The status
bar shows `-- SENTENCE FOCUS --` with the current line. See
`docs/commands-sentence-focus-mode.md` for the full list.

Note: decimal points and abbreviations (`3.14`, `Mr.`) are treated as sentence
terminators — a deliberate simplification.

Customize sentence-focus-mode keys with a `[sentence_focus_keys]` table (see
`docs/config.md`); it overlays the normal bindings while sentence focus mode is
active.

## Paragraph focus mode

Press `cp` (`paragraph_focus_enter`) to switch to **paragraph focus mode**, where
a whole paragraph (a block of content lines) is highlighted:

| Keys | Command |
|---|---|
| `h`, `k`, `<Left>`, `<Up>` | `paragraph_focus_prev` — previous paragraph |
| `l`, `j`, `<Right>`, `<Down>` | `paragraph_focus_next` — next paragraph |
| `<Esc>` | `paragraph_focus_exit` — back to normal mode |

Paragraphs are detected from the line layout: consecutive lines form a paragraph
until a larger-than-normal vertical gap or a column change. As with sentence
focus, all of `hjkl`/arrows collapse to previous/next (no Vim `{`/`}` aliases),
motion wraps across pages, and counts work (`3j`). Every other binding still
works; scroll / page-jump commands carry the highlight to visible content while
zoom leaves it in place. The status bar shows `-- PARAGRAPH FOCUS --` with the
current line range. See `docs/commands-paragraph-focus-mode.md` for the full list.

Customize paragraph-focus-mode keys with a `[paragraph_focus_keys]` table (see
`docs/config.md`); it overlays the normal bindings while paragraph focus mode is
active.

## Customizing

Add a `[keys]` table to your config file (see `docs/config.md` for its
location). Entries **add to or override** the defaults — list only your
changes:

```toml
[keys]
"J"     = "scroll_half_page_down"  # rebind a default
"<C-o>" = "open_file"              # add a new binding
```

Invalid entries (bad key syntax or unknown command names) are reported in
the status bar at startup, with all other bindings staying functional.
Valid command names are listed in `docs/commands.md`.
