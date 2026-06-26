# Sentence focus mode commands

**Sentence focus mode** is entered with `cs` (`sentence_focus_enter`) from normal
mode. It highlights one sentence at a time — a run of cells ending at
sentence-terminating punctuation (`.`, `!`, `?`) plus any trailing closing
quotes/brackets. A sentence may span several lines and is drawn as a
text-selection shape. The status bar shows `-- SENTENCE FOCUS --` with the
highlighted sentence's starting line. Press `<Esc>` (`sentence_focus_exit`) to
return to normal mode; the highlighted sentence is remembered.

Counts work here too (`3l`, `2h`).

Note: decimal points and abbreviations (`3.14`, `Mr.`) are treated as sentence
terminators — a deliberate simplification.

## Sentence motion

Sentences are a linear sequence, so all of `hjkl` and the arrow keys collapse to
previous/next (there are no Vim `(`/`)` aliases).

| Command | Effect | Count |
|---|---|---|
| `sentence_focus_exit` | leave sentence focus mode (the sentence is remembered) | - |
| `sentence_focus_prev` | move the highlight to the previous sentence | repeats N times |
| `sentence_focus_next` | move the highlight to the next sentence | repeats N times |

`h`/`k`/`<Left>`/`<Up>` move to the previous sentence and
`l`/`j`/`<Right>`/`<Down>` to the next. Motion wraps across pages. The view
auto-scrolls to keep the highlighted sentence on screen as it moves.

## Inherited view commands

Every normal-mode command stays available in sentence focus mode with its normal
binding — only `hjkl`, the arrow keys and `<Esc>` are remapped (to sentence
motion / exit). So the page-scroll, page-navigation and zoom commands all work
here too.

**Scroll and page jumps reposition the highlight.** After any scroll or
page-jump command, the highlight jumps to the sentence containing the top-most
content line now visible in the window.

**Zoom keeps the highlight in place.** Zoom changes magnification around the
window center; the highlight stays on the same sentence.

The application commands `open_file`, `quit` and `cancel` also keep their
normal-mode behavior. See `docs/commands-normal-mode.md` for those.

## Customizing

Sentence-focus bindings live in the `[sentence_focus_keys]` config table, which
overlays the normal `[keys]` while sentence focus mode is active. See
`docs/config.md` and `docs/keybindings.md`.
