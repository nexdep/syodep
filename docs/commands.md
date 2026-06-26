# Commands

Every user-visible action in syodep is a *command*. Keybindings map key
sequences to command names (see `docs/keybindings.md`); future features
(command palette, text objects) reuse the same registry.

syodep has four input modes, each with its own command page:

- **[Normal mode](commands-normal-mode.md)** — the default. `hjkl` scroll
  the page; covers scrolling, page navigation, zoom, entering the focus
  modes, and the application commands.
- **[Caret focus mode](commands-caret-focus-mode.md)** — entered with `cc`.
  `hjkl` move a modal cursor (the caret) through the document content; the
  view, page-navigation and zoom commands stay available, and scroll / page
  jumps carry the caret along.
- **[Line focus mode](commands-line-focus-mode.md)** — entered with `cl`.
  A whole line is highlighted; `j`/`k` move it line by line and `h`/`l` move
  between columns on multi-column pages. The view, page-navigation and zoom
  commands stay available, and scroll / page jumps carry the highlight along.
- **[Word focus mode](commands-word-focus-mode.md)** - entered with `cw`.
  A whole Vim-like word run is highlighted; `h`/`b` and `l`/`w` move between
  word runs, while `j`/`k` move line-wise. The view, page-navigation and zoom
  commands stay available, and scroll / page jumps carry the highlight along.

Counts: most commands accept a count prefix typed before the binding
(`5j`, `3J`, `12G`). Where a count has a special meaning it is noted on the
per-mode page.

## Planned (not yet implemented)

Phase 2 adds selection/highlight/search/bookmark/mark/jump commands;
phase 3 adds text-object commands (`select_word`, `highlight_sentence`,
…) and smart jump. See `docs/roadmap.md`.
