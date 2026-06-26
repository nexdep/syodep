# Development log

Newest entries first. Each entry records what was implemented, the tests
that cover it, and decisions worth remembering. Future contributors (human
or agent): read `docs/architecture.md` first, then the latest entries here,
then `docs/roadmap.md` for what to build next.

---

## 2026-06-26 — Sentence focus & paragraph focus modes

### Implemented

- **Sentence focus mode** (`syodep-core`): added `Mode::SentenceFocus`, entered
  with `cs` (`sentence_focus_enter`) and left with `<Esc>`. It highlights a whole
  sentence (`SentenceMark { page, start_line, start_cell, end_line, end_cell }`),
  which may span several lines but never crosses a page. Boundaries are detected
  over the cell stream at sentence-terminating punctuation (`.`/`!`/`?`, via
  `is_sentence_terminator`) plus trailing closing quotes/brackets
  (`is_sentence_trailer`), reusing the caret's cross-line `next_cell`/`prev_cell`
  walkers.
- **Paragraph focus mode**: added `Mode::ParagraphFocus`, entered with `cp`
  (`paragraph_focus_enter`). It highlights a block of lines
  (`ParagraphMark { page, start_line, end_line }`). The pure `paragraph_segments`
  splits a page's lines on column changes (reusing `column_ranges`/
  `column_index_of`) and on vertical gaps larger than `PARAGRAPH_GAP_FACTOR`
  times the median line height.
- **Navigation**: both modes are a linear sequence, so all of `hjkl` and the
  arrow keys collapse to previous/next (`*_focus_prev`/`*_focus_next`); counts
  repeat the motion and motion wraps across pages. Scroll and page jumps carry
  the highlight to visible content; zoom leaves it in place.
- **Config/FFI/Qt**: added `[sentence_focus_keys]` and `[paragraph_focus_keys]`
  overlay tables. Paragraph reuses the single-rect `SyoCaret` path
  (`syo_app_paragraph`, purple Qt highlight); sentence renders a text-selection
  shape via a new `syo_app_sentence`/`syo_sentence_free` array FFI
  (`SyoRect`/`SyoSentence`) drawn as one red rectangle per spanned line.
- **Docs**: added the sentence- and paragraph-focus command pages, keybinding/
  config references, and docs-check coverage for the new commands and default
  bindings.

### Tests

- Pure `caret` tests cover the sentence classifiers and `paragraph_segments`
  (tight grouping, large-gap split, column-change split, single/empty).
- App-level tests cover enter/mark/status, next/prev stepping, a sentence
  spanning lines (multi-rect), cross-page motion, within-page paragraph
  stepping, exit behavior and no-document safety.
- FFI round-trip toggles paragraph validity and exercises the sentence rect
  array + `syo_sentence_free`.

### Decisions

- Marks are **page-confined** (every overlay goes through the per-page
  `page_rect_to_screen`); navigation crosses pages while a single mark never
  straddles one, matching `WordMark`/`LineMark`.
- Decimal points and abbreviations (`3.14`, `Mr.`) are treated as sentence
  terminators — a deliberate v1 simplification.

---

## 2026-06-26 — Word focus mode

### Implemented

- **Word focus mode** (`syodep-core`): added `Mode::WordFocus`, entered with
  `cw` (`word_focus_enter`) and left with `<Esc>`. It highlights a whole
  Vim-like word run (`WordMark { page, line, start_cell, end_cell }`), using
  the same word classes as caret word motions: letters/digits/underscore
  together, punctuation/symbols as separate runs, whitespace skipped and each
  image as one stop.
- **Navigation**: `h`/`b` move to the previous run, `l`/`w` move to the next,
  and `j`/`k` move line-wise while keeping a goal column. Counts repeat the
  motion. Scroll and page jumps carry the highlight to visible content; zoom
  leaves it on the same word.
- **Config/FFI/Qt**: added `[word_focus_keys]` with default overlay semantics,
  `syo_app_word` for the overlay rectangle and a green Qt highlight distinct
  from caret and line focus.
- **Docs**: added the word-focus command page, keybinding/config references
  and docs-check coverage for the new commands and default bindings.

### Tests

- Config test covers `[word_focus_keys]` default merging and user overrides.
- App-level tests cover enter/mark/status, horizontal and vertical motion
  across lines/pages, inherited bindings, exit behavior and no-document safety.
- Docs check covers the new command page and word-focus default bindings.

---

## 2026-06-25 — Graphics diagnostics & WSL auto-fallback (0.3.0)

### Implemented

- **Self-diagnosing graphics startup** (`ui-qt/src/diagnostics.{h,cpp}`): a new
  module that detects the host platform (OS, WSL via `WSL_DISTRO_NAME` or
  `/proc/version`, GPU passthrough via `/dev/dxg`, X11/Wayland display env) and,
  **before the `QApplication` is constructed**, applies safe fallbacks: on WSL
  it forces `QT_QPA_PLATFORM=xcb` when a display is present (the WSLg
  wayland-egl client buffer integration is routinely empty) and
  `Qt::AA_UseSoftwareOpenGL` when there is no GPU passthrough. Any user-set
  `QT_QPA_PLATFORM`/`LIBGL_ALWAYS_SOFTWARE`/`QT_OPENGL` is respected and left
  untouched. Silent on a normal launch.
- **`syodep --check`**: prints platform detection, the selected Qt platform
  plugin and the reason, a live OpenGL probe (offscreen context →
  `GL_RENDERER`/`GL_VERSION`, so software `llvmpipe` is visible), the config
  file path with loaded/not-found state plus parse warnings
  (`syo_app_startup_warnings`), and version info; then exits.
- **Extended `syodep --version`**: shell, core, Qt, platform and build-type
  lines instead of the bare name+version. Handled before `QApplication`, so it
  needs no display.
- **FFI**: added `syo_core_version()` (`crates/syodep-ffi/src/lib.rs`) returning
  the core crate version; freed with the existing `syo_string_free`.

### Why

Launching in WSL crashed with `wayland-egl` integration failures and
`QOpenGLWidget: Failed to create context`, because the canvas is a
`QOpenGLWidget` requiring a GL context the WSLg environment could not provide.
The app now degrades automatically instead of failing, and `--check` makes the
active graphics path inspectable.

### Tests

- `QT_QPA_PLATFORM=offscreen ./build/ui-qt/syodep --smoke-test f.pdf` still
  passes; `--version` and `--check` exercised manually (offscreen for the GL
  probe in headless CI). Detection logic is pure and reads only env/filesystem
  signals.

### Decisions

- Fallback selection is **heuristic**, not a live GPU probe: Qt locks the
  platform plugin and GL backend at `QApplication` construction, so there is no
  context to probe at decision time.
- `Qt::AA_UseSoftwareOpenGL` is the cross-platform software switch (Mesa
  llvmpipe on Linux, `opengl32sw` on Windows); the `xcb` override is Linux-only.

---

## 2026-06-25 — Caret word motions

### Implemented

- **Word motions in caret focus mode** (`syodep-core`): added
  `caret_focus_next_word`, `caret_focus_end_word` and
  `caret_focus_prev_word`, bound by default to `w`, `e` and `b` in
  `[caret_focus_keys]`. Motions use Vim-like lowercase word runs:
  letters/digits/underscore together, punctuation/symbols as separate runs,
  whitespace skipped, line/page boundaries splitting runs, and each image as
  one word-like stop. Counts repeat the motion; the caret goal column is
  refreshed after landing and the view scrolls to keep the caret visible.
- **Docs/config**: command docs, default keybindings and the caret-focus
  config example now include the word-motion bindings.

### Tests

- Pure `caret.rs` tests cover word classification, skipped whitespace,
  punctuation runs, line-boundary splitting and image single-stop behavior.
- App-level tests cover `w`, `e`, `b`, repeated counts across lines/pages,
  document-edge clamping and image cells as word-motion stops.

---

## 2026-06-25 — Line focus mode

### Implemented

- **Line focus mode** (`syodep-core`): a third input mode (`Mode::LineFocus`)
  alongside Normal and CaretFocus, entered with `cl` (`line_focus_enter`) and
  left with `<Esc>`. It highlights a whole content line (`ContentLine.bbox`);
  `j`/`k` move the highlight line by line, wrapping across pages, and `h`/`l`
  move between columns on multi-column pages. It mirrors the caret machinery at
  line granularity: a `LineMark { page, line }` position, a `line_focus_keymap`
  (normal keymap overlaid with `[line_focus_keys]`), `enter_line_focus` /
  `line_move` / `line_step_up`/`down` / `line_step_column`, viewport-follow via
  `reposition_line_to_viewport` (reusing `topmost_visible_line`),
  `ensure_line_visible`, and `line_screen_rect`. Scroll/page jumps carry the
  highlight; zoom leaves it in place — same rules as the caret.
- **Column detection** (`caret.rs`, pure + unit-tested): `column_ranges`
  greedily clusters a page's line bboxes into disjoint horizontal bands;
  `column_index_of` maps a line to its column; `nearest_line_in_column` is the
  goal-row analogue of `nearest_cell_in_line` so `h`/`l` keep the vertical
  position. `h`/`l` are a no-op on single-column pages and edge columns.
- **Entry binding `cl`, not `ll`**: `ll` would make a lone `l` ambiguous (both a
  binding and a prefix), breaking `l` scrolling and caret-right. `cl` reuses the
  prefix-only `c` focus family (`cc` caret, `cl` line) with no collisions.
- **FFI + Qt**: `syo_app_line` returns the highlight rect (reusing `SyoCaret`'s
  layout); the Qt canvas paints it as a translucent amber band, distinct from
  the blue caret. Header regenerates via cbindgen.
- **Docs**: new `docs/commands-line-focus-mode.md`; `[line_focus_keys]` in
  `docs/config.md`; line-focus section in `docs/keybindings.md`;
  `scripts/check-docs.sh` extended to cover the new page and bindings.

### Tests

A two-column PDF fixture (`test_support::pdf_two_column_page`) plus core tests:
enter/mark/status, vertical page crossing, exit restores scrolling, inherited
bindings carry the mark, `h`/`l` no-op on single column and jump columns on the
fixture; FFI validity test for `syo_app_line`; pure tests for the three column
helpers.

---

## 2026-06-25 — Navigation commands in caret focus mode

### Implemented

- **View commands carry the caret** (`syodep-core`): the page-scroll
  (`scroll_half_page_down/up`, `scroll_page_down/up`), page-navigation
  (`next_page`, `prev_page`, `goto_first_page`, `goto_last_page`) and zoom
  (`zoom_in/out`, `fit_width`, `zoom_reset`) commands are now first-class in
  caret focus mode. They were already *reachable* there (the caret-focus
  keymap is the normal keymap plus the `[caret_focus_keys]` overlay, which
  only remaps `hjkl`/arrows/`<Esc>`, so no clashes), but the caret stayed
  put. Now scroll and page jumps reposition the caret to the top-most content
  visible in the new viewport, keeping its goal column; zoom leaves the caret
  in place. New `App::reposition_caret_to_viewport` + `topmost_visible_line`
  hook into `App::execute` after the view mutates (`app.rs`); they reuse
  `View::scroll`, `DocumentLayout::page_at_y`/`page`, and `ContentLine::bbox`.
- **Per-mode command docs**: `docs/commands.md` is now an index linking
  `docs/commands-normal-mode.md` and `docs/commands-caret-focus-mode.md`. The
  caret-focus page documents the inherited view commands and the
  reposition/zoom behavior. `scripts/check-docs.sh` greps command names
  against the per-mode pages (and its caret check now follows the renamed
  `default_caret_focus_keybindings`).

### Tests

- `caret_focus_page_jumps_carry_the_caret` (J/K/G/gg move the caret onto the
  destination page), `caret_focus_page_scroll_advances_the_caret` (`<C-f>`
  advances the caret), `caret_focus_zoom_leaves_the_caret_in_place`
  (`+`/`zw` keep the caret), and an extended
  `caret_focus_keeps_non_hjkl_bindings`.

---

## 2026-06-17 — AppImage Qt platform plugin bundling

### Implemented

- **Wayland platform support**: the Linux AppImage release job now installs
  `qt6-wayland` in the Ubuntu 22.04 build container and explicitly asks
  `linuxdeploy-plugin-qt` to bundle `libqwayland-egl.so` and
  `libqwayland-generic.so` alongside the existing offscreen plugin.
  `libqxcb.so` remains the plugin's default platform backend.
- **Packaging verification**: after building the AppImage, CI extracts it
  and asserts the bundled Qt platform directory contains `xcb`,
  `offscreen`, and both Wayland platform plugins before running the
  smoke test.
- **Docs**: `docs/packaging.md` now records the `qt6-wayland` dependency
  and the extracted-AppImage plugin check.

### Test strategy

Workflow/docs change. Local verification: `git diff --check` and
`./scripts/check-docs.sh`; the full AppImage extraction check and packaged
offscreen smoke test run in GitHub Actions on the next release workflow.

---

## 2026-06-17 — Continuous prerelease downloads

### Implemented

- **Rolling release**: `.github/workflows/release.yml` now also runs on
  pushes to `main` and reuses the existing AppImage and Windows zip builders.
  After both packages pass their smoke tests, `publish-continuous` updates
  the `continuous` tag and prerelease with stable asset names for the latest
  main build.
- **Release boundary**: `vMAJOR.MINOR.PATCH` tags still create immutable
  versioned releases and bump the Scoop manifest. The rolling prerelease is
  marked as a prerelease and does not update Scoop metadata.
- **Docs**: `AGENTS.md` and `docs/packaging.md` now document the split
  between branch CI artifacts, the continuous prerelease, and versioned
  releases.

### Test strategy

Workflow/docs change. Local verification: `git diff --check` and
`./scripts/check-docs.sh`; the package smoke tests and continuous release
publish path run in GitHub Actions on the next `main` push.

---

## 2026-06-17 — Push build artifacts policy

### Implemented

- **CI push artifacts**: `.github/workflows/ci.yml` now explicitly runs on
  branch pushes and `v*` tags. A new `build-artifact` job waits for the
  existing lint, Rust test, Qt smoke-test, and docs jobs, then builds a
  release-mode Linux binary, packages it as a tarball with `version.txt`,
  uploads the SHA-256 checksum, and retains the workflow artifact for 14 days.
- **Release boundary**: public releases remain owned by
  `.github/workflows/release.yml` on `vMAJOR.MINOR.PATCH` tags; branch pushes
  produce ephemeral CI artifacts only.
- **Agent guidance**: `AGENTS.md` now records the branch-artifact/tag-release
  policy and forbids CI-driven version bump commits or checked-in binaries.

### Test strategy

Docs/workflow-only change. Local verification: `git diff --check` and
`./scripts/check-docs.sh`; the build artifact path is enforced by the updated
GitHub Actions dependency graph on the next push.

---

## 2026-06-16 — Modal caret navigation (text + images)

### Implemented

- **Content-geometry layer** (`syodep-pdf`): `Document::page_content` returns
  per-page `ContentLine`s of `Cell`s — one cell per character (bbox from the
  glyph quad) and one cell per image — in reading order, in page points.
  Uses `TextPageFlags::PRESERVE_IMAGES` (image blocks are dropped by the
  default stext flags). Image vs text blocks are discriminated via
  `block.image()`/`block.lines()` since `TextBlockType` is not re-exported by
  the bindings.
- **Modal caret** (`syodep-core`): a new `Mode { Normal, CaretFocus }` plus a
  `caret.rs` module (position, direction, goal-column cell picker). `c`
  enters caret focus mode; `h`/`l` move the caret character-wise (wrapping across
  lines/pages), `j`/`k` line-wise keeping a goal column; `<Esc>` exits. Each
  image is a single stop. The view auto-scrolls to keep the caret visible
  (`View::scroll_doc_rect_into_view`), and page content is cached per page in
  the session. The caret keymap is the normal keymap cloned with the
  `[caret_focus_keys]` overrides applied (`Keymap::overlay`), so every other
  binding still works in caret focus mode and normal-binding errors are reported
  once.
- **Config**: new `[caret_focus_keys]` table (`h/j/k/l`/arrows + `<Esc>` defaults)
  and a `cc = caret_focus_enter` default in `[keys]`.
- **FFI/shell**: `SyoCaret` + `syo_app_caret` project the caret rect (canvas
  pixels) across the C ABI; `CanvasWidget::paintGL` draws a translucent
  accent box with a border. The status bar shows `-- CARET FOCUS --  Ln L, Col C`.

### Test strategy

TDD for the pure pieces: `caret.rs` goal-column picker; `View`
`page_rect_to_screen`/`scroll_doc_rect_into_view`; `syodep-pdf` content
extraction including an image cell from a new `pdf_with_image` fixture
(generated, not checked in). App-level integration tests cover enter/exit,
character/line motion, page wrapping, goal-column preservation across pages,
counts, and that non-`hjkl` bindings still work in caret focus mode. The FFI
round-trip test enters caret focus mode, moves, and exits. 104 tests total (was
88); Qt shell covered by compile + offscreen smoke test as before.

### Decisions (details in `docs/architecture.md`, row 11)

- Modal caret (mode-selected keymap) over an always-on caret: keeps `hjkl`
  scrolling intact and matches the existing Vim-like modal design.
- One caret stop per image; goal-column vertical motion like a text editor.
- `page_content` runs only in caret focus mode and is cached, so plain reading is
  unaffected.

### Known limitations / next steps

- Word/sentence/paragraph text objects and selection build on this caret
  (phase 2/3). The caret position is not yet persisted across sessions.
- RTL/vertical scripts rely on MuPDF reading order; not specially handled.

---

## 2026-06-12 — Linux AppImage release

### Implemented

- **`release-build-linux`** (release.yml) now produces
  `syodep-x86_64.AppImage` instead of an unpackaged binary. It builds in
  an `ubuntu:22.04` container (the AppImage inherits the build machine's
  glibc floor — 2.35 covers Ubuntu 22.04+/Debian 12+/Fedora 36+), with
  distro Qt 6.2 and rustup-installed Rust, then packages with
  `linuxdeploy` + `linuxdeploy-plugin-qt` (run via
  `--appimage-extract-and-run`; containers have no FUSE).
- **`packaging/`**: `syodep.desktop` (Office;Viewer, application/pdf
  MIME) and a placeholder `syodep.svg` icon, both required by
  linuxdeploy.
- The `offscreen` Qt platform plugin is bundled
  (`EXTRA_PLATFORM_PLUGINS`) so the AppImage itself is smoke-tested in
  CI (offscreen render of a generated PDF) — same fail-in-CI principle
  as the Windows staged smoke test.
- **`publish-release`** attaches `syodep-vX.Y.Z-x86_64.AppImage` to
  GitHub releases alongside the Windows zip.

### Test strategy

CI-only: the AppImage smoke test exercises open + render through the
real packaged binary. Additionally verified by downloading the artifact
from a `workflow_dispatch` run and running the smoke test on a local
machine with a different userland than the build container.

---

## 2026-06-12 — Scoop distribution

### Implemented

- **`bucket/syodep.json`**: the repo doubles as a Scoop bucket
  (`scoop bucket add syodep https://github.com/nexdep/syodep`). The
  manifest points at the GitHub release zip, sets `extract_dir`
  (`syodep-win64`), `bin`, a Start Menu shortcut, and
  `checkver`/`autoupdate` metadata. No `persist` entries: user data lives
  in `%APPDATA%`, not the install dir.
- **`publish-release`** (release.yml) now bumps the manifest after
  creating each release: recomputes the zip's SHA256, rewrites
  `version`/`url`/`hash` with `jq`, commits to `main` as
  `github-actions[bot]`. The job checks out `main` (not the tag) for this.

### Test strategy

Manifest JSON validated with `jq`; the hash was computed from the actual
published v0.1.0 asset. The CI bump path only executes on the next `v*`
tag — verify it then (winget was considered and dropped for now).

---

## 2026-06-11 — Windows link fixes + GitHub releases on tag push

### Implemented

- Fixed the Windows shell link, found by reading CI logs after four
  distinct failures (all in the top-level `CMakeLists.txt`):
  1. strip `/defaultlib:` linker-flag tokens from rustc's
     `native-static-libs` output (CMake treated them as file paths);
  2. strip ANSI color codes from that output (`CARGO_TERM_COLOR=always`
     in CI poisons tokens) — note `\x` escapes are invalid in CMake
     strings, use `string(ASCII 27 …)`;
  3. resolve `libmupdf.lib`/`libthirdparty.lib` to their cargo `OUT_DIR`
     paths — on Windows rustc does **not** bundle them into the staticlib
     (unlike Linux), leaving 382 unresolved `fz_*` symbols;
  4. configure the Windows CI shell build as Release — a debug config
     links debug Qt + `/MDd` against MuPDF's `/MD` objects (LNK2038).
- **`publish-release`** (release.yml): on `v*` tag pushes, downloads the
  portable zip artifact and publishes it to a GitHub release as
  `syodep-vX.Y.Z-win64.zip` (`gh release create --generate-notes`,
  `contents: write` permission). Manual dispatch runs still stop at
  workflow artifacts.

### Test strategy

CI-only changes, verified by watching runs to green: full CI (all six
jobs including both Windows jobs), a `workflow_dispatch` release run
producing a working zip, and a `v*` tag push producing a GitHub release.

---

## 2026-06-11 — Windows binary in CI/CD

### Implemented

- **`qt-build-windows`** (ci.yml): builds the Qt shell on `windows-2022`
  on every push/PR — Qt 6.7.3 via `jurplel/install-qt-action`
  (`win64_msvc2019_64`), MSVC env via `ilammy/msvc-dev-cmd`,
  `cmake -G Ninja`, then the offscreen smoke test. The exe is a
  GUI-subsystem binary, so the smoke test asserts the exit code (stdout is
  invisible on Windows).
- **`release-build-windows`** (release.yml): release-mode build, portable
  tree staged with `windeployqt --release --no-translations` plus LICENSE/
  README/sample config, smoke test re-run from the staged tree with Qt
  stripped from PATH (an incomplete DLL bundle fails in CI, not on a user
  machine), `syodep-win64.zip` uploaded as a workflow artifact.

### Test strategy

CI-only change: no core logic touched, so no new Rust tests. The Windows
smoke test (build + open + render through the real exe) plus the staged
PATH-stripped smoke test are the appropriate coverage. Verified by pushing
and watching the GitHub Actions runs to green, plus a `workflow_dispatch`
release run producing a working zip.

### Notes / remaining

- Qt version is pinned (6.7.3) in both workflows; bump deliberately.
- Still planned (docs/packaging.md): Linux AppImage, NSIS installer,
  attaching artifacts to GitHub releases on tag push.

---

## 2026-06-11 — Milestone 1: MVP foundation

Everything below landed as one milestone, built bottom-up in small slices
(config → core input/layout → storage → pdf backend → App integration →
FFI → Qt shell → build system → CI/docs).

### Implemented

- **Workspace layout**: Cargo workspace with `syodep-config`,
  `syodep-core`, `syodep-pdf`, `syodep-storage`, `syodep-ffi`; Qt shell in
  `ui-qt/`; top-level CMake driving cargo + Qt.
- **syodep-config**: TOML config (`[view]`, `[keys]`), defaults, overlay
  semantics for user keybindings, descriptive parse errors (unknown field,
  type mismatch, file context), and the key-chord syntax/parser
  (`gg`, `<C-d>`, `<C-A-Left>`, named keys).
- **syodep-core**:
  - `Command` registry (19 commands) with name round-tripping.
  - Input state machine: keymap trie, count prefixes (`5j`, `120G`,
    Vim-style `0` rule), multi-key sequences, deterministic prefix
    disambiguation, Escape-cancels-pending, per-entry error reporting for
    invalid bindings.
  - Layout/View: document-space page stacking with gaps and centering,
    clamped scrolling (small docs centered), current-page = window center,
    page navigation, zoom anchored at the window center with limits,
    fit-width, visible-page computation.
  - Byte-bounded LRU render cache keyed by (page, quantized scale).
  - `App`: ties everything together; `Effects {redraw, quit,
    open_file_dialog}` out; position autosave after navigation + on drop.
- **syodep-pdf**: safe wrapper over the `mupdf` crate exposing only
  syodep types (`Document`, `Size`, `Bitmap` RGBA8, `OutlineItem`);
  open-from-path/bytes, page sizes, render-at-scale with white background,
  plain-text extraction, outline; password-protected files rejected with a
  clear error. Includes a programmatic PDF fixture builder
  (`test_support`, also used by other crates and CI).
- **syodep-storage**: rusqlite (bundled), migration runner over
  `PRAGMA user_version` (refuses newer-schema DBs), schema v1
  (`documents` keyed by SHA-256 content fingerprint, `positions`),
  position save/load, cascade delete.
- **syodep-ffi**: panic-safe C ABI (`syo_app_*`), cbindgen-generated
  header, explicit free functions for strings/bitmaps, default
  config/db path helpers (XDG / %APPDATA%).
- **ui-qt**: `MainWindow` (status bar, file dialog, owns the core handle),
  `CanvasWidget` (QOpenGLWidget; paints core-provided bitmaps, forwards
  keys/wheel/resize), `key_encoder` (QKeyEvent → chord strings),
  `--smoke-test` mode for CI.
- **Build**: top-level CMake builds the Rust staticlib via cargo and links
  the Qt shell against it (Linux: + fontconfig/freetype; Windows libs
  prepared). `SYODEP_RUST_PROFILE` defaults to release.
- **CI**: lint (fmt, clippy -D warnings), tests on Linux + Windows, Qt
  build + offscreen smoke test, docs-consistency script
  (`scripts/check-docs.sh`). Release workflow placeholder with the real
  pipeline specified in `docs/packaging.md`.
- **Docs**: README + architecture/commands/keybindings/config/testing/
  packaging/roadmap/this log.

### Test strategy actually used

TDD for the pure crates (tests written with/before the code, all pure
logic covered without I/O where possible); integration tests at the App
and FFI levels; generated PDF fixtures instead of binary files; offscreen
smoke test for the shell. 88 tests at milestone close. Deviation from
strict TDD: the Qt shell itself is covered by compilation + smoke test
only, by design (it contains no logic).

### Decisions (details in `docs/architecture.md`)

- `mupdf-rs` bindings instead of hand-rolled bindgen (reproducible
  Windows/Linux builds; unsafe stays out of our tree).
- Content-fingerprint document identity (survives moves/renames).
- Scroll state stored in document space → zoom-stable.
- Timer-free key disambiguation (wait on ambiguous prefix; Esc cancels).
- Synchronous rendering for M1; async/tiles deferred to phase 3.

### Known limitations / next steps

- Rendering is synchronous on the UI thread; large pages at high zoom can
  stutter. Planned: phase 3 async tiles (the `App::render_page` seam stays).
- `visible_pages` FFI is capped at 64 entries by the shell's stack buffer
  (fine until extreme zoom-out; the API already reports the real count).
- No text selection yet — phase 2 starts with the char-geometry text layer.
- ~~Windows CI builds the Rust workspace but not yet the Qt shell~~
  (done: see "Windows binary in CI/CD" entry above).

---

*(log started 2026-06-11)*
