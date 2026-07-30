# Highlight mode commands

**Highlight mode** turns a selection into a highlight. It is entered with `a`
from focus mode or from visual mode: whatever is focused or selected becomes a
pending highlight in the highlight colour, and stays adjustable. The status bar
shows `-- HIGHLIGHT (scope) --` with the highlighted line range.

A pending highlight *is* a selection — it has the same two ends, each with its
own scope, and every visual-mode motion reshapes it. That is not a coincidence
of the implementation, it is the design: `[highlight_keys]` binds `hjkl`, the
arrows, `w`/`e`/`b`/`s`/`p`, `o` and `o` plus a scope letter to the very same
`visual_*` commands, so a key cannot mean one thing while selecting and another
while highlighting. See `docs/commands-visual-mode.md` for what each of them
does.

Entering from focus mode gives the highlight a second end at the same place, so
a highlight that started on one focused word can still be grown by `w` or `o`.

Counts work here too (`3l`, `2w`).

## Keeping or discarding

| Command | Effect | Count |
|---|---|---|
| `highlight_enter` | turn the focus highlight or selection into a pending highlight | - |
| `highlight_commit` | store the highlight and return to visual mode, still selected | - |
| `highlight_discard` | throw the highlight away, restoring the mode and selection `a` was pressed on | - |

Bound to `a` (from focus and visual mode), `a` again (to keep it), and `<Esc>`
or `<BS>` (to throw it away).

`highlight_discard` restores everything: the mode, the position, the scope and
the anchor, exactly as they were when `a` was pressed — including undoing any
motions made while the highlight was pending. Entering from focus mode
synthesised the second end, so discarding removes it again and focus mode is not
left holding a selection.

**`a` is unbound in normal mode.** There is nothing selected there — only a
remembered position — so `a` would highlight whatever you last happened to look
at.

## Leaving into another mode

`v` and `c`, with or without a scope letter, **keep** the highlight and go to
the mode they name. They are not bound in `[highlight_keys]` at all: they fall
through to the normal `[keys]` table's `visual_enter*` and `focus_enter*`, which
store the pending highlight on the way out.

| Keys | Effect |
|---|---|
| `a` | keep it, back to visual mode with the same text selected |
| `v` | keep it, back to visual mode with the same text selected |
| `vw` (`vc`/`ve`/`vs`/`vp`) | keep it, visual mode with the moving end at that scope |
| `c`, `cw` (`cc`/`ce`/`cs`/`cp`) | keep it, focus mode at that scope on the moving end |
| `<leader>w` | keep it, then save the document |
| `<Esc>` or `<BS>` | discard it |

Unlike a bare `v` inside visual mode, `v` here does *not* collapse the selection
onto the moving end: the two ends are the ones you just shaped, so throwing that
away would be surprising. The only way to lose a pending highlight is to ask for
it.

## Storing

`highlight_commit` writes the highlight to syodep's database (the `highlights`
table), keyed by the document's content fingerprint, so it comes back the next
time the document is opened. Stored highlights are drawn from their geometry
alone, which means they appear immediately on open without any page content
having to be extracted first.

Storing records the covered text as well as the geometry, for the notes and
export features on the roadmap.

If the database cannot be written the highlight still exists for the session and
can still be saved into the PDF; the failure is reported in the status bar rather
than refusing to highlight.

## Saving into the PDF

`save_document` (`<leader>w`, leader `<Space>` by default) overwrites the open
PDF with every stored highlight embedded as a real PDF `Highlight` annotation,
so other readers show them too. It is a normal-mode command available in every
mode — see `docs/commands-normal-mode.md`.

## Inherited view commands

Every normal-mode command stays available with its normal binding; only the keys
listed in `[highlight_keys]` are remapped. So page scrolling, page navigation
and zoom all work while a highlight is pending, and — as in visual mode — they
leave the highlight where it is.

## Customizing

Highlight-mode bindings live in the `[highlight_keys]` config table, which
overlays the normal `[keys]` while a highlight is pending. The colour is
`[view] highlight_color` and `highlight_opacity`. See `docs/config.md` and
`docs/keybindings.md`.
