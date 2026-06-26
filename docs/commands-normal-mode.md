# Normal mode commands

**Normal mode** is the default mode: `hjkl` (and the arrow keys) scroll the
page. Press `cc` (`caret_focus_enter`) to switch to **caret focus mode** —
see `docs/commands-caret-focus-mode.md`.

Every user-visible action is a *command*. Keybindings map key sequences to
command names (see `docs/keybindings.md`); future features (command palette,
text objects) reuse the same registry.

Counts: most commands accept a count prefix typed before the binding
(`5j`, `3J`, `12G`). Where a count has a special meaning it is noted below.

## Scrolling

| Command | Effect | Count |
|---|---|---|
| `scroll_down` | scroll down by `view.scroll_step` pixels | multiplies step |
| `scroll_up` | scroll up by `view.scroll_step` pixels | multiplies step |
| `scroll_left` | scroll left by `view.horizontal_scroll_step` pixels | multiplies step |
| `scroll_right` | scroll right by `view.horizontal_scroll_step` pixels | multiplies step |
| `scroll_half_page_down` | scroll down half a window | multiplies |
| `scroll_half_page_up` | scroll up half a window | multiplies |
| `scroll_page_down` | scroll down a full window | multiplies |
| `scroll_page_up` | scroll up a full window | multiplies |

Scrolling is clamped to the document; documents smaller than the window are
centered.

## Page navigation

| Command | Effect | Count |
|---|---|---|
| `next_page` | jump to the top of the next page | advances N pages |
| `prev_page` | jump to the top of the previous page | goes back N pages |
| `goto_first_page` | go to the first page | **with count N: go to page N** (1-based) |
| `goto_last_page` | go to the last page | **with count N: go to page N** (1-based) |

The "current page" is the page under the center of the window.

## Zoom

| Command | Effect | Count |
|---|---|---|
| `zoom_in` | multiply zoom by `view.zoom_step` | applies N times |
| `zoom_out` | divide zoom by `view.zoom_step` | applies N times |
| `fit_width` | fit the widest page to the window width | — |
| `zoom_reset` | set zoom to 100% (72 dpi) | — |

Zoom keeps the document point at the window center fixed and is clamped to
5%–1600%.

## Focus modes

| Command | Effect | Count |
|---|---|---|
| `caret_focus_enter` | enter caret focus mode, placing the caret on the nearest content | — |
| `line_focus_enter` | enter line focus mode, highlighting the nearest content line | — |
| `word_focus_enter` | enter word focus mode, highlighting the first visible word run | — |

See `docs/commands-caret-focus-mode.md` for everything available once in
caret focus mode, `docs/commands-line-focus-mode.md` for line focus mode and
`docs/commands-word-focus-mode.md` for word focus mode.

## Application

| Command | Effect |
|---|---|
| `open_file` | open the native file picker and load the chosen PDF |
| `quit` | save the reading position and quit |
| `cancel` | clear pending count/sequence input (bound to `<Esc>`; Esc also clears pending input implicitly mid-sequence) |

## Planned (not yet implemented)

Phase 2 adds selection/highlight/search/bookmark/mark/jump commands;
phase 3 adds text-object commands (`select_word`, `highlight_sentence`,
…) and smart jump. See `docs/roadmap.md`.
