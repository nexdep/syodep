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

Application:

| Keys | Command |
|---|---|
| `o` | `open_file` |
| `q` | `quit` |
| `<Esc>` | `cancel` |

The mouse wheel (and horizontal trackpad scrolling) also scrolls the view;
this is a convenience, not the primary workflow.

## Caret focus mode

syodep has two input modes. In **normal mode** (the default) `hjkl` scroll
the page. Press `cc` (`caret_focus_enter`) to switch to **caret focus mode**, where a
cursor moves through the document's content — text characters and images:

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
