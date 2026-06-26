# Word focus mode commands

**Word focus mode** is entered with `cw` (`word_focus_enter`) from normal
mode. It highlights one Vim-like word run at a time: letters/digits/underscore
form word runs, punctuation/symbols form separate runs, whitespace is skipped,
and each image is a single stop. The status bar shows `-- WORD FOCUS --` with
the highlighted word's line and column. Press `<Esc>` (`word_focus_exit`) to
return to normal mode; the highlighted word is remembered.

Counts work here too (`3w`, `2j`).

## Word motion

| Command | Effect | Count |
|---|---|---|
| `word_focus_exit` | leave word focus mode (the word is remembered) | - |
| `word_focus_left` | move the highlight to the previous word run | repeats N times |
| `word_focus_right` | move the highlight to the next word run | repeats N times |
| `word_focus_up` | move one line up, landing on the word nearest the goal column | repeats N times |
| `word_focus_down` | move one line down, landing on the word nearest the goal column | repeats N times |

`h`/`b` move left and `l`/`w` move right. Line motion wraps across pages,
keeping a goal column like caret focus mode. The view auto-scrolls to keep the
highlighted word on screen as it moves.

## Inherited view commands

Every normal-mode command stays available in word focus mode with its normal
binding - only `hjkl`, `w`/`b`, the arrow keys and `<Esc>` are remapped (to
word motion / exit). So the page-scroll, page-navigation and zoom commands all
work here too.

**Scroll and page jumps reposition the highlight.** After any scroll or
page-jump command, the highlighted word jumps to the top-most content line now
visible in the window, landing near the remembered goal column.

**Zoom keeps the highlight in place.** Zoom changes magnification around the
window center; the highlight stays on the same word.

The application commands `open_file`, `quit` and `cancel` also keep their
normal-mode behavior. See `docs/commands-normal-mode.md` for those.

## Customizing

Word-focus bindings live in the `[word_focus_keys]` config table, which
overlays the normal `[keys]` while word focus mode is active. See
`docs/config.md` and `docs/keybindings.md`.
