# Normal mode commands

**Normal mode** is the default mode: `hjkl` (and the arrow keys) scroll the
page. Press `fc` (`focus_enter_char`) to switch to **focus mode** — see
`docs/commands-focus-mode.md`.

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
| `center_view` | scroll so the remembered focus position is at the viewport center | — |

Zoom keeps the document point at the window center fixed and is clamped to
5%–1600%. `center_view` is a no-op in normal mode when there is no remembered
focus position; in focus/visual/highlight it centers the current highlight
(true centering, ignoring `view.scroll_off`).

## Focus mode

Focus mode highlights one position in the document's content. Which unit it
covers is the *scope*, chosen when entering — there is one focus mode with five
granularities, not five modes.

| Command | Effect | Count |
|---|---|---|
| `focus_enter_char` | focus character by character, on the nearest content | — |
| `focus_enter_word` | focus word by word, on the first visible word run | — |
| `focus_enter_line` | focus line by line, on the nearest content line | — |
| `focus_enter_sentence` | focus sentence by sentence, on the nearest sentence | — |
| `focus_enter_paragraph` | focus paragraph by paragraph, on the nearest paragraph | — |

These also change the scope from *inside* focus mode, without moving the
highlight. See `docs/commands-focus-mode.md` for everything available once
focused.

## Application

| Command | Effect |
|---|---|
| `open_file` | open the native file picker and load the chosen PDF |
| `save_document` | overwrite the open PDF with its highlights embedded |
| `toggle_highlights_sidebar` | toggle or activate the Highlights sidebar page |
| `toggle_annotations_sidebar` | toggle or activate the Annotations sidebar page |
| `create_annotation` | capture the current Focus/Visual/Highlight source as a pending Markdown annotation and open the editor (rejected in Normal mode) |
| `quit` | save the reading position and quit; asks first if there are highlights not yet saved to the PDF |
| `cancel` | clear pending count/sequence input (bound to `<Esc>`; Esc also clears pending input implicitly mid-sequence) |

### `save_document`

Bound to `<leader>w` (leader `<Space>` by default), and available in every mode
— saving is not modal. It writes every stored highlight into the PDF as a real
`Highlight` annotation, so other readers show them too, and reports what it did
in the status bar. A highlight still being placed is kept first, as `a` would.

The file is **overwritten in place**, with no backup: the new PDF is written
beside the original and renamed over it, so an interrupted save can never leave a
half-written file where the document was. If the write fails the original is
untouched, the document stays open, and the error is shown in the status bar.

Two consequences worth knowing:

- Rewriting changes the file's content hash, and syodep keys documents by hash so
  their state survives moves and renames. The document's row is moved to the new
  hash as part of the save, so the reading position carries over.
- Once the highlights are in the PDF, syodep stops drawing them as an overlay and
  its own records of them are dropped — the renderer draws the annotations
  themselves, and drawing both would paint them twice. They keep the same
  colour and opacity: `highlight_opacity` is written into the saved
  annotation, and the live overlay already previews it with the same Multiply
  blending every reader uses for a highlight, so a highlight looks the same
  before and after saving.

Saving with nothing to save leaves the file completely alone.

### `toggle_highlights_sidebar`

Bound to `<leader>a` and available in every mode, including with no document
open — the sidebar has an empty state, and a binding that silently does nothing
depending on hidden state is worse than one that shows it.

The core only *asks*: whether a panel is on screen is the shell's business.
`MainWindow::toggleSidebarPage(Highlights)` shows the Highlights page, substitutes
it for Annotations if that page is visible, or hides the dock when Highlights is
already active. Opening focuses the Highlights list; hiding returns focus to the
canvas. `View → Highlights` runs the same code.

### `toggle_annotations_sidebar`

Bound to `<leader>n`. Mirrors `toggle_highlights_sidebar` for the Annotations
page of the same dock. Cross-toggling substitutes the page without hiding the
dock; self-toggling hides it.

### `create_annotation`

Bound to `n` in every mode. In Focus, Visual, or Highlight mode it captures an
immutable `DocumentAnchor` from the current focus unit or selection, stores it as
`pending_annotation_anchor`, and asks the shell to open the Annotations page in
creation mode — never a toggle, never hides the dock, and never commits a
highlight. In Normal mode it does nothing except report:

> Enter Focus, Visual, or Highlight mode to create an annotation.

Empty Save is rejected; cancel creates nothing. The Markdown body is stored
exactly as entered once non-empty validation passes.

### `quit`

Bound to `<leader>q` — bare `q` does nothing, so a single careless keystroke
cannot lose anything. Like `save_document`, a highlight still being placed is
kept first, as `a` would.

If quitting would leave any highlight not yet embedded in the PDF — one just
committed, or one still being placed — the shell asks first: **Save & Quit**,
**Discard & Quit**, or **Cancel**. "Discard" does not delete anything: the
highlight is already recorded in the database, independently of the PDF file,
and simply comes back as a pending overlay the next time this document is
opened. A failed save chosen from this dialog behaves exactly like a failed
`save_document` — the document stays open and the error appears in the status
bar.

The window's own close button (and Alt+F4) is protected the same way.

## Planned (not yet implemented)

Phase 2 adds search/bookmark/mark/jump commands on top of the selection visual
mode provides (mouse selection is still to come); phase 3 adds text-object
commands (`select_word`, `highlight_sentence`, …) and smart jump. See
`docs/roadmap.md`.
