# Architecture

## Overview

syodep is split into a UI-independent Rust core and a thin Qt 6 shell,
joined by a small C ABI:

```
 ┌────────────────────────── ui-qt (C++, Qt 6) ──────────────────────────┐
 │ MainWindow ── status line, file dialog                                │
 │ CanvasWidget (QOpenGLWidget) ── paints bitmaps, forwards input        │
 │ key_encoder ── QKeyEvent → "j" / "G" / "<C-d>" strings                │
 └──────────────────────────────┬────────────────────────────────────────┘
                                │ C ABI (crates/syodep-ffi, cbindgen header)
 ┌──────────────────────────────┴────────────────────────────────────────┐
 │ syodep-core: App                                                      │
 │   command system ─ input state machine (counts, sequences, keymap)    │
 │   layout/View ─ document space, scroll, zoom, visible pages           │
 │   render cache ─ byte-bounded LRU of page bitmaps                     │
 ├───────────────┬──────────────────────────┬────────────────────────────┤
 │ syodep-config │ syodep-pdf               │ syodep-storage             │
 │ TOML, chords  │ safe MuPDF wrapper       │ SQLite + migrations        │
 └───────────────┴──────────────────────────┴────────────────────────────┘
```

**The rule that everything else follows from:** the core never depends on
Qt types; the shell never contains document logic. The shell forwards
events and paints what the core tells it to paint.

## Crates

### syodep-config

Owns the *shape* of the TOML config and the textual key-chord syntax
(`gg`, `<C-d>`, `<Esc>`), which is shared by the config file and the shell's
key encoder. Semantic validation of command names lives in `syodep-core`
(which owns the command set); this keeps the dependency direction
`core → config` with no cycles.

Error philosophy: configuration errors are *never* fatal. `Config::load`
returns either a config or an error with file/field context; callers fall
back to defaults and surface the message (status bar). User `[keys]`
entries overlay the built-in defaults rather than replacing them.

### syodep-core

Pure Rust, fully unit-testable without a display or a real file (layout and
input take plain data). Key pieces:

- **`Command`** (`command.rs`): every user action is a variant; keybindings
  map key sequences to command *names*. UI never calls behavior directly.
  This is the seed of the later command palette and text-object commands.
- **Input state machine** (`input.rs`): keymap trie + pending state
  (count prefix, partial sequence). Disambiguation: a sequence that is both
  a binding and a prefix of a longer binding waits for more input; `<Esc>`
  cancels. If the wait ends in an unbound sequence, the longest bound prefix
  fires and the leftover chords are queued for replay, so a binding that is
  also a prefix stays reachable. Timer-free throughout, hence deterministic
  and easily tested. The replay queue is drained by `App::handle_key` rather
  than resolved inside `InputState`, because the fired command may switch
  modes and the replayed chords must use the new mode's keymap.
- **Layout** (`layout.rs`): pages stacked vertically in *document space*
  (PDF points), centered on the widest page. Scroll offsets are stored in
  document space so they survive zoom changes. `View` provides clamped
  scrolling, page navigation, anchor-preserving zoom, fit-width and the
  visible-page computation.
- **Render cache** (`render_cache.rs`): LRU keyed by (page, quantized
  scale), bounded by bytes. Rendering itself is synchronous on the UI
  thread in milestone 1; asynchronous tile rendering is a later milestone
  (see roadmap) and will live behind the same `App::render_page` seam.
- **`App`** (`app.rs`): glues everything; input events come in, `Effects`
  (redraw / quit / open-file-dialog) come out. Persists the reading
  position after every navigation command and on drop.
- **Focus mode** (`caret.rs` + `app.rs`): one highlighted position over
  page-content geometry. `Mode` has exactly three variants — `Normal`, `Focus`,
  `Visual` — and *granularity is not a mode*: `Focus` carries a `Scope` (char,
  word, line, sentence or paragraph). The app stores one `Caret` plus that
  scope; what is drawn is derived by `scope_span(caret, scope)` and cached in
  `focus_span`, so changing the scope reinterprets the position you are on
  rather than restoring a separate per-granularity mark. (It used to be five
  `Mode` variants with five marks, which meant `cw` then `ce` teleported you to
  wherever you last were in line focus. The bug is unrepresentable now.)
  The focus keymap is the normal keymap plus the `[focus_keys]` overrides, so
  every other command still works while focused. Extracted page content is
  cached per page in the session; the pure goal-column and word-boundary
  helpers live in `caret.rs`.
- **Visual mode** (`caret.rs` + `app.rs`): a two-ended selection over the same
  content layer. It stores only the anchored end (`VisualAnchor`); the *moving*
  end is the app's `focus`/`focus_scope`. There is exactly one "where you are"
  in the whole app, and visual mode adds a second endpoint rather than a second
  position. `o` exchanges the anchor with the focus position, scopes included,
  so there is no separate "which end is active" flag to drift. Rendering expands
  each end by its own scope and takes the outermost edges, which makes the ends
  crossing a non-case and `o` provably invisible.
  `VisualSelection` still exists, but as a *read-only view* assembled by
  `visual_selection()` — it is what the tests and status line read, never what
  the app stores.
- **Why the moving end is not stored separately**: it was, and that was a bug.
  `enter_focus` (the `c` chords) read `focus` while `hjkl` moved
  `visual.head`, and the two only reconciled inside `exit_visual` — so leaving
  visual mode any other way silently restored the pre-selection position. The
  fix was to delete the duplicate rather than sync it: `exit_visual` now just
  drops the anchor, and `enter_focus` needs no visual-aware branch at all.
- **Why focus and visual share almost everything**: a focus highlight is a
  selection whose two ends coincide. Both resolve to an inclusive `(Caret,
  Caret)` span, so one `scope_span` derives the extent, one `step_scope` table
  maps (scope, direction) onto a motion, and one `span_screen_rects` turns a
  span into overlay rectangles. A scope therefore cannot mean one thing in one
  mode and something else in the other — enforced by construction rather than
  by discipline. The FFI reflects this: a single `SyoOverlay` shape serves both,
  fetched with `syo_app_focus` / `syo_app_selection`. Overlays are drawn per
  *visible* page, so cost does not grow with span length.
  The two stay separate modes because their exit semantics differ, focus draws
  one end where visual draws two, and they want distinct colours — but they
  share the position itself.

### syodep-pdf

The only crate that touches MuPDF. Wraps the maintained `mupdf` crate
(bindings + vendored MuPDF C sources) and exposes syodep-owned types only:
`Document`, `Size`, `Rect`, `Bitmap` (tightly packed RGBA8), `OutlineItem`,
and the content-geometry layer `ContentLine`/`Cell` (per-page text/image
boxes from `page_content`, the foundation the caret — and later selection
and search — navigate). No MuPDF type or pointer crosses this boundary.

**Decision — use `mupdf-rs` instead of hand-rolled bindgen FFI:** building
MuPDF from vendored source via cargo gives reproducible Linux+Windows
builds with zero system dependencies, and the bindings already encapsulate
the unsafe context/pointer management. If we outgrow them, the swap stays
inside this crate. Trade-off: slightly less control over MuPDF build flags.

Threading: MuPDF contexts are thread-local; `Document` is `!Send` and all
rendering happens on the opening thread (milestone 1 renders synchronously).

### syodep-storage

SQLite via `rusqlite` (bundled SQLite, no system dependency). Decisions:

- **Fingerprint identity:** documents are keyed by SHA-256 of file content,
  not path, so positions/annotations survive file moves and renames. The
  path is stored and refreshed for display purposes.
- **Migrations from day one:** `PRAGMA user_version` counts applied entries
  of an append-only `MIGRATIONS` list; each runs in a transaction.
  Databases from a *newer* build are refused rather than corrupted.
- **No dynamic state in TOML:** TOML is for human-edited settings only.

Schema v1: `documents` (id, fingerprint UNIQUE, path, timestamps) and
`positions` (document_id PK→documents CASCADE, scroll_x, scroll_y, zoom).
Phase 2 adds marks/bookmarks/highlights/notes tables as new migrations.

### syodep-ffi

The C ABI. Owns nothing conceptually; it is a mechanical projection of
`App` plus panic containment (`catch_unwind` on every entry point — a Rust
panic must never unwind into C++). Strings/bitmaps returned to C++ are
heap copies with explicit `syo_*_free` functions. The header is generated
by cbindgen at build time into `crates/syodep-ffi/include/syodep_ffi.h`.

### ui-qt

Four small files; intentionally boring:

- `key_encoder` translates `QKeyEvent` to the chord syntax (the shell's
  only input knowledge).
- `CanvasWidget` (a `QOpenGLWidget`) forwards keys/wheel/resizes, asks the
  core for visible page rects + bitmaps, paints them with `QPainter` on the
  GL-backed surface, and draws the caret overlay rectangle the core reports
  (`syo_app_caret`, in canvas pixels) on top. It keeps a tiny per-page
  `QImage` cache only to avoid re-copying bitmaps across the FFI every
  repaint; the real cache is in the core. Tiled GL texture rendering is
  planned for phase 3 (roadmap).
- `MainWindow` owns the `SyoApp*` handle, the status label and the native
  file dialog, and accepts PDFs dropped onto the window. The canvas fills the
  window but leaves `acceptDrops()` false, so Qt delivers drag events to the
  window; only it needs the flag.
- `main.cpp` parses the CLI and implements `--smoke-test` for CI.

## Data flow example: pressing `5j`

1. Qt delivers two key events; `key_encoder` produces `"5"`, `"j"`.
2. Shell calls `syo_app_key_event` for each; core's `InputState` buffers
   the count, then resolves `j` → `scroll_down` with count 5.
3. `App::execute` scrolls the `View` by 5 × `scroll_step` pixels (converted
   to document space, clamped), saves the position to SQLite.
4. The FFI returns `SYO_EFFECT_REDRAW`; the shell calls `update()` and
   refreshes the status line from `syo_app_status_text`.
5. `paintGL` asks for visible pages, fetches bitmaps (core render cache),
   draws them.

## Decisions log

| # | Decision | Why | Revisit when |
|---|----------|-----|--------------|
| 1 | Rust core + thin Qt shell over C ABI | testability, no Qt types in core, clean ownership | never (foundational) |
| 2 | `mupdf-rs` bindings instead of own bindgen layer | reproducible cross-platform builds, less unsafe to own | MuPDF features we can't reach |
| 3 | Content fingerprint (SHA-256) as document identity | state survives moves/renames | huge files make hashing slow → partial hash |
| 4 | Scroll state in document space (points) | zoom changes don't displace the view | — |
| 5 | Timer-free key disambiguation (prefix waits, then longest-prefix fallback + replay) | predictability, testability; a binding that is also a prefix stays reachable without a timer | users demand Vim `timeoutlen` |
| 12 | Visual-mode scope belongs to each *endpoint*, not to the start/end role | crossing the anchor and coming back is the identity, so the selection never silently changes shape on an overshoot | a use case needs "the first edge is always line-granular" |
| 13 | Selection overlay is computed per visible page, not per selected page | cost is O(visible lines) however long the selection is, and page content is never force-extracted off-screen | selections need to be exported/persisted whole (then resolve the span separately from drawing it) |
| 6 | Synchronous rendering + byte-bounded LRU cache | simplest correct thing for M1 | phase 3 (async tiles) |
| 7 | Counts are runtime input, not part of binding syntax | matches Vim; keeps keymap finite | — |
| 8 | `0` counts only after a nonzero digit (Vim rule) | lets `0`-prefixed bindings exist later | — |
| 9 | Config errors degrade to defaults + warning | app must always start | — |
| 10 | cbindgen-generated header, checked into neither repo nor docs | single source of truth in Rust | ABI freeze for plugins (not planned) |
| 11 | Modal caret over content geometry (mode-selected keymap) | Vim-like `hjkl` caret without losing `hjkl` scrolling; one stop per image; goal-column vertical motion | always-on caret, or richer text objects (phase 3) |

## Sioyek: conceptual inspirations (clean-room)

Recorded per the project's license policy — these are *ideas* observed from
using Sioyek and reading its documentation, re-designed and re-implemented
independently:

- keyboard-first reading loop and command abstraction
- multi-key sequences with count prefixes
- persistent per-document reading state
- planned for later phases: marks, jump history, smart jump, overview popup

Deliberately avoided implementation patterns (also from studying Sioyek's
architecture at a high level): god Document/MainWidget classes, Qt types in
core logic, raw pointer ownership spread across the app, `void*` config
values, ad-hoc global state.
