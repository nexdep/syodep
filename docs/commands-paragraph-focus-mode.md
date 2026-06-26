# Paragraph focus mode commands

**Paragraph focus mode** is entered with `cp` (`paragraph_focus_enter`) from
normal mode. It highlights one paragraph at a time — a block of consecutive
content lines, drawn as a single rectangle. Paragraphs are detected from the
line layout: consecutive lines form a paragraph until a larger-than-normal
vertical gap or a column change. The status bar shows `-- PARAGRAPH FOCUS --`
with the highlighted paragraph's line range. Press `<Esc>`
(`paragraph_focus_exit`) to return to normal mode; the highlighted paragraph is
remembered.

Counts work here too (`3j`, `2k`).

## Paragraph motion

Paragraphs are a linear sequence, so all of `hjkl` and the arrow keys collapse
to previous/next (there are no Vim `{`/`}` aliases).

| Command | Effect | Count |
|---|---|---|
| `paragraph_focus_exit` | leave paragraph focus mode (the paragraph is remembered) | - |
| `paragraph_focus_prev` | move the highlight to the previous paragraph | repeats N times |
| `paragraph_focus_next` | move the highlight to the next paragraph | repeats N times |

`h`/`k`/`<Left>`/`<Up>` move to the previous paragraph and
`l`/`j`/`<Right>`/`<Down>` to the next. Motion wraps across pages. The view
auto-scrolls to keep the highlighted paragraph on screen as it moves.

## Inherited view commands

Every normal-mode command stays available in paragraph focus mode with its
normal binding — only `hjkl`, the arrow keys and `<Esc>` are remapped (to
paragraph motion / exit). So the page-scroll, page-navigation and zoom commands
all work here too.

**Scroll and page jumps reposition the highlight.** After any scroll or
page-jump command, the highlight jumps to the paragraph containing the top-most
content line now visible in the window.

**Zoom keeps the highlight in place.** Zoom changes magnification around the
window center; the highlight stays on the same paragraph.

The application commands `open_file`, `quit` and `cancel` also keep their
normal-mode behavior. See `docs/commands-normal-mode.md` for those.

## Customizing

Paragraph-focus bindings live in the `[paragraph_focus_keys]` config table, which
overlays the normal `[keys]` while paragraph focus mode is active. See
`docs/config.md` and `docs/keybindings.md`.
