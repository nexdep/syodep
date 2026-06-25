# Caret focus mode commands

**Caret focus mode** is entered with `cc` (`caret_focus_enter`) from normal
mode. A modal cursor (the *caret*) moves through the document's content —
text characters and images — independently of scrolling. Each image is a
single caret stop. The status bar shows `-- CARET FOCUS --` with the
caret's line and column. Press `<Esc>` (`caret_focus_exit`) to return to
normal mode; the caret position is remembered.

Counts work here too (`5l`, `3j`).

## Caret motion

| Command | Effect | Count |
|---|---|---|
| `caret_focus_exit` | leave caret focus mode (the caret position is remembered) | — |
| `caret_focus_left` | move the caret one character left (wraps to the previous line/page) | repeats N times |
| `caret_focus_right` | move the caret one character right (wraps to the next line/page) | repeats N times |
| `caret_focus_up` | move the caret one line up, keeping its column | repeats N times |
| `caret_focus_down` | move the caret one line down, keeping its column | repeats N times |

The view auto-scrolls to keep the caret on screen as it moves.

## Inherited view commands

Every normal-mode command stays available in caret focus mode with its
normal binding — only `hjkl`, the arrow keys and `<Esc>` are remapped (to
caret motion / exit). So the page-scroll, page-navigation and zoom commands
below all work here too. (The plain line-scroll commands `scroll_down` /
`scroll_up` / `scroll_left` / `scroll_right` are *not* reachable from the
keyboard, since their default `hjkl`/arrow bindings move the caret instead.)

**Scroll and page jumps reposition the caret.** After any of these
commands, the caret jumps to the top-most content now visible in the window,
keeping its goal column — so the caret follows the scroll instead of being
left behind off-screen.

| Command | Effect | Count |
|---|---|---|
| `scroll_half_page_down` | scroll down half a window, then move the caret to the top of the new view | multiplies |
| `scroll_half_page_up` | scroll up half a window, then move the caret to the top of the new view | multiplies |
| `scroll_page_down` | scroll down a full window, then move the caret to the top of the new view | multiplies |
| `scroll_page_up` | scroll up a full window, then move the caret to the top of the new view | multiplies |
| `next_page` | jump to the next page; the caret moves onto it | advances N pages |
| `prev_page` | jump to the previous page; the caret moves onto it | goes back N pages |
| `goto_first_page` | go to the first page; the caret moves onto it | **with count N: page N** (1-based) |
| `goto_last_page` | go to the last page; the caret moves onto it | **with count N: page N** (1-based) |

**Zoom keeps the caret in place.** Zoom changes magnification around the
window center; the caret stays on the same content.

| Command | Effect | Count |
|---|---|---|
| `zoom_in` | multiply zoom by `view.zoom_step` | applies N times |
| `zoom_out` | divide zoom by `view.zoom_step` | applies N times |
| `fit_width` | fit the widest page to the window width | — |
| `zoom_reset` | set zoom to 100% (72 dpi) | — |

The application commands `open_file`, `quit` and `cancel` also keep their
normal-mode behavior. See `docs/commands-normal-mode.md` for those.

## Customizing

Caret-focus bindings live in the `[caret_focus_keys]` config table, which
overlays the normal `[keys]` while caret focus mode is active. See
`docs/config.md` and `docs/keybindings.md`.
