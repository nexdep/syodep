# Roadmap

Status legend: ✅ done · 🚧 in progress · ⬜ planned

## Phase 1 — MVP foundation (milestone 1) ✅

- ✅ Rust core / Qt shell split over a C ABI
- ✅ Open a local PDF (CLI, `<leader>o`, file dialog)
- ✅ Render the entire PDF in a continuous scrollable view (MuPDF)
- ✅ Keyboard navigation: scroll, half/full pages, next/prev page,
  first/last page, `{n}G`, count prefixes
- ✅ Zoom in/out, fit-to-width, zoom reset
- ✅ Text extraction API for pages
- ✅ TOML config + keybinding overlay, graceful error handling
- ✅ SQLite database with versioned migrations
- ✅ Save/restore last reading position (fingerprint-keyed)
- ✅ Render cache (byte-bounded LRU)
- ✅ Tests (687), CI (lint, Linux+Windows tests, Qt build, smoke test,
  docs checks), documentation set

## Phase 2 — selection, annotation, search ⬜

Ordered roughly by dependency:

1. ✅ Character/image-geometry content layer (per-page character + image
   boxes from `syodep-pdf::Document::page_content`; foundation for
   everything below)
2. 🚧 Keyboard text selection + selection overlay rendering (done: visual
   mode, `v`/`vc`/`vw`/`ve`/`vs`/`vp`, two independently-scoped ends, `o` to
   switch ends); mouse selection still to come
3. ✅ Highlight selected text (highlight mode, `a`); SQLite `highlights` table
   (migrations v2–v3); Pending overlays on reload; `<leader>w` embeds Pending
   highlights as PDF annotations and keeps the rows as Embedded (stable ids,
   summaries, reveal-by-id, Markdown export, C ABI list)
4. ⬜ Search within document; result overlays; `/` and next/previous result
   bindings (not bare `n`/`N` — those are reserved for annotations and must use
   different chords when search lands)
5. ✅ Markdown annotations anchored to selected text (independent of highlights;
   `text_annotations` migration v4; `n` creates from Focus/Visual/Highlight;
   `<leader>n` toggles the Annotations sidebar page; Step 8 stabilizes dirty
   prompts, id-based selection, export format, and persistence-off behavior)
6. ⬜ Bookmarks (current position) and single-key marks (`m{a-z}`,
   `'{a-z}`)
7. ⬜ Jump history: jump-back / jump-forward (`<C-o>` / `<C-i>`)
8. ⬜ Fuzzy search over highlights
9. 🚧 Export annotations to Markdown and JSON — Markdown is done for highlights
   and text annotations (`# Highlights` / `# Annotations` with `## Page`
   sections; clipboard + `.md` file export). JSON export is not implemented
10. ✅ Annotation sidebar (Qt) — one fixed-right dock with mutually exclusive
    Highlights and Annotations pages (`<leader>a` / `<leader>n`), keyboard-driven
    lists, focus-safe effective toggle bindings, Escape-to-close with preserved
    drafts, Markdown annotation editing, clipboard copy and file export. Agent
    chat, filtering, and external PDF annotation import remain planned.

Delivered ahead of the rest: a **modal caret** (`c` to enter, then `hjkl`)
navigates the content layer character- and line-wise across text and images,
auto-scrolling to follow, plus word/line/sentence/paragraph scopes over
the same layer. **Visual mode** (item 2) builds a two-ended selection on top of
them, and was the seam **highlight mode** (item 3) built on: it reuses visual
mode's motion *commands* outright, so highlighting a range and selecting one are
the same code. See the development log.

## Phase 3 — text objects and smart navigation ⬜

- ✅ Text objects: word / sentence / paragraph over the text layer
  (focus modes, and as visual-mode selection scopes)
- ✅ Tables, images and equations as navigable units: detected in the content
  layer and treated as one stop from line scope up — and drawn as a single box
  — in focus, visual and highlight mode; an image is one stop at word scope too
- ✅ Headings as sentence/paragraph units: detected from type size and weight,
  so a heading is one step for `s` and `p` while `w` still walks its words
- ✅ Display equations as units: detected from math fonts and math characters on
  a line set apart from the prose, so an aligned system is one step for `e`, `s`
  and `p` and tints as one box, while `w` and `h`/`l` still walk through it
- ✅ Page furniture kept out of the caret's path: running heads and folios by
  cross-page repetition, sideways stamps and watermarks by baseline direction
- ✅ Footnotes detected from the bottom margin band and undersized type, one
  stop from line scope up like a table (reachable only by deliberate word/char
  motion, never walked line by line); `s`/`p` additionally skip a footnote
  entirely while reading through a page's body, so it never interrupts
  ordinary reading
- ✅ Captions detected from proximity to figures/tables plus `Fig.`/`Table`
  prefixes; one stop from line scope up; `s`/`p` skip them like footnotes
- ✅ Code blocks detected from monospace font share; one stop from line scope
  up (not auto-skipped by `s`/`p`)
- ✅ CJK sentence terminators (`。！？．`) recognised; full CJK word
  segmentation remains out of scope
- ⬜ Cross-page sentences/paragraphs (marks are page-local by design today —
  see architecture; would need mark shapes → `(Caret, Caret)`, cross-page
  expansion, and a paragraph join heuristic)
- ⬜ Text-object selection, highlighting, annotation (`viw`-style
  composability on top of the existing command/count system)
- ⬜ Smart jump to references, figures, tables, equations (the table, image and
  equation primitives now exist in the content layer; the jump motion does not)
- ⬜ Overview popup for jump targets; candidate navigation for ambiguous
  targets
- ⬜ Bibliography/reference detection
- ⬜ Async/tiled rendering with GL textures; prefetch neighboring pages
- ⬜ Explicit reading-order rewrite (MuPDF stream order is documented;
  interleaved-column fixtures characterise current behaviour only)

## Infrastructure milestones 🚧

- 🚧 Packaging (spec in `docs/packaging.md`):
  - ✅ Windows CI build + smoke test on every push/PR
  - ✅ Windows portable zip release artifact (windeployqt, staged smoke test)
  - ✅ Linux AppImage (ubuntu:22.04 container build, linuxdeploy + Qt plugin)
  - ✅ Windows NSIS installer (per-user, silent-capable, opt-in PDF handler)
  - ⬜ Windows code signing (unsigned builds trip SmartScreen)
  - ✅ attach artifacts to GitHub releases on tag push
  - ✅ Scoop bucket, auto-bumped: `bucket/syodep.json` on release tags,
    `bucket/syodep-continuous.json` on every main push
- ✅ Linux Wayland-only shell with a probed OpenGL canvas, shared-code raster
  fallback, and Weston-based CI/AppImage smoke tests
- ⬜ Command palette (`:` / `<C-p>`) listing the command registry
- ✅ Modal keybinding-help overlay (`<C-?>`) showing effective bindings for all modes
- ⬜ Config hot-reload

## Explicitly out of scope

Mobile/touch, SyncTeX/LaTeX integration, paper downloading, web search,
embedded JS runtime, TTS, presentation mode, portals, freehand drawing,
cloud sync, AI summaries, reference-manager features, browser/Electron/
Tauri shells.
