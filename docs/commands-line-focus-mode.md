# Line focus mode commands

**Line focus mode** is entered with `cl` (`line_focus_enter`) from normal
mode. It highlights a whole content line and moves that highlight line by
line, independently of scrolling. On multi-column pages, `h`/`l` move the
highlight between columns. The status bar shows `-- LINE FOCUS --` with the
highlighted line number. Press `<Esc>` (`line_focus_exit`) to return to
normal mode; the highlighted line is remembered.

Counts work here too (`3j`).

## Line motion

| Command | Effect | Count |
|---|---|---|
| `line_focus_exit` | leave line focus mode (the line is remembered) | — |
| `line_focus_up` | move the highlight one line up (wraps to the previous page) | repeats N times |
| `line_focus_down` | move the highlight one line down (wraps to the next page) | repeats N times |
| `line_focus_left` | move to the line in the previous column, keeping the row | repeats N times |
| `line_focus_right` | move to the line in the next column, keeping the row | repeats N times |

`h`/`l` are a no-op on single-column pages and at the edge column. Columns
are detected automatically from the page's line layout: a page with two or
more disjoint horizontal text bands is treated as multi-column. When moving
between columns the highlight lands on the line nearest the current
vertical position (a "goal row", mirroring the caret's goal column). The
view auto-scrolls to keep the highlighted line on screen as it moves.

## Inherited view commands

Every normal-mode command stays available in line focus mode with its
normal binding — only `hjkl`, the arrow keys and `<Esc>` are remapped (to
line motion / exit). So the page-scroll, page-navigation and zoom commands
all work here too, exactly as in caret focus mode.

**Scroll and page jumps reposition the highlight.** After any scroll or
page-jump command, the highlight jumps to the top-most content line now
visible in the window — so it follows the scroll instead of being left
behind off-screen.

**Zoom keeps the highlight in place.** Zoom changes magnification around the
window center; the highlight stays on the same line.

The application commands `open_file`, `quit` and `cancel` also keep their
normal-mode behavior. See `docs/commands-normal-mode.md` for those.

## Customizing

Line-focus bindings live in the `[line_focus_keys]` config table, which
overlays the normal `[keys]` while line focus mode is active. See
`docs/config.md` and `docs/keybindings.md`.
