# Roadmap

Status legend: ✅ done · 🚧 in progress · ⬜ planned

## Phase 1 — MVP foundation (milestone 1) ✅

- ✅ Rust core / Qt shell split over a C ABI
- ✅ Open a local PDF (CLI, `o`, file dialog)
- ✅ Render the entire PDF in a continuous scrollable view (MuPDF)
- ✅ Keyboard navigation: scroll, half/full pages, next/prev page,
  first/last page, `{n}G`, count prefixes
- ✅ Zoom in/out, fit-to-width, zoom reset
- ✅ Text extraction API for pages
- ✅ TOML config + keybinding overlay, graceful error handling
- ✅ SQLite database with versioned migrations
- ✅ Save/restore last reading position (fingerprint-keyed)
- ✅ Render cache (byte-bounded LRU)
- ✅ Tests (88), CI (lint, Linux+Windows tests, Qt build, smoke test,
  docs checks), documentation set

## Phase 2 — selection, annotation, search ⬜

Ordered roughly by dependency:

1. ⬜ Character-geometry text layer (per-page char/word boxes from
   `syodep-pdf` TextPage; foundation for everything below)
2. ⬜ Mouse + keyboard text selection; selection overlay rendering
3. ⬜ Highlight selected text; SQLite `highlights` table (migration v2);
   highlight overlays rendered on reload
4. ⬜ Search within document; result overlays; `/`, `n`, `N`
5. ⬜ Text notes attached to highlights
6. ⬜ Bookmarks (current position) and single-key marks (`m{a-z}`,
   `'{a-z}`)
7. ⬜ Jump history: jump-back / jump-forward (`<C-o>` / `<C-i>`)
8. ⬜ Fuzzy search over highlights and notes
9. ⬜ Export annotations to Markdown and JSON
10. ⬜ Annotation sidebar (Qt, read-only first)

## Phase 3 — text objects and smart navigation ⬜

- ⬜ Text objects: word / sentence / paragraph over the text layer
- ⬜ Text-object selection, highlighting, annotation (`viw`-style
  composability on top of the existing command/count system)
- ⬜ Smart jump to references, figures, tables, equations
- ⬜ Overview popup for jump targets; candidate navigation for ambiguous
  targets
- ⬜ Bibliography/reference detection
- ⬜ Async/tiled rendering with GL textures; prefetch neighboring pages

## Infrastructure milestones 🚧

- 🚧 Packaging (spec in `docs/packaging.md`):
  - ✅ Windows CI build + smoke test on every push/PR
  - ✅ Windows portable zip release artifact (windeployqt, staged smoke test)
  - ⬜ Linux AppImage
  - ⬜ Windows NSIS installer
  - ⬜ attach artifacts to GitHub releases on tag push
- ⬜ Command palette (`:` / `<C-p>`) listing the command registry
- ⬜ Config hot-reload

## Explicitly out of scope

Mobile/touch, SyncTeX/LaTeX integration, paper downloading, web search,
embedded JS runtime, TTS, presentation mode, portals, freehand drawing,
cloud sync, AI summaries, reference-manager features, browser/Electron/
Tauri shells.
