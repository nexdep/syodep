# Commands

Every user-visible action in syodep is a *command*. Keybindings map key
sequences to command names (see `docs/keybindings.md`); future features
(command palette, text objects) reuse the same registry.

syodep has three input modes, each with its own command page:

- **[Normal mode](commands-normal-mode.md)** — the default. `hjkl` scroll
  the page; covers scrolling, page navigation, zoom, entering focus mode, and
  the application commands.
- **[Focus mode](commands-focus-mode.md)** — entered with `cc`, `cw`, `ce`,
  `cs` or `cp`. One position in the document content is highlighted and `hjkl`
  move it. How much a step covers is the **scope** — a character, word, line,
  sentence or paragraph — which is a setting of the mode, not a mode of its
  own: the same five chords change it in place while you stay focused. The
  view, page-navigation and zoom commands stay available, and scroll / page
  jumps carry the highlight along.
- **[Visual mode](commands-visual-mode.md)** — entered with `v` (or `vc`/`ve`/
  `vw`/`vs`/`vp`). Selects a *range* rather than a single unit: motions move one
  end while the other stays anchored, `o` switches which end moves, and each end
  has its own scope. A bare `v` inherits the focus scope. Unlike focus mode,
  scroll and page jumps leave the selection where it is.

Focus and visual are the same idea at different arities — a focus highlight is
a selection whose two ends coincide — so they share one per-scope motion table
and one set of key meanings. Learning one teaches the other.

Counts: most commands accept a count prefix typed before the binding
(`5j`, `3J`, `12G`). Where a count has a special meaning it is noted on the
per-mode page.

## Planned (not yet implemented)

Phase 2 adds highlight/search/bookmark/mark/jump commands on top of the
selection visual mode provides (mouse selection is still to come); phase 3
adds text-object commands (`select_word`, `highlight_sentence`, …) and smart
jump. See `docs/roadmap.md`.
