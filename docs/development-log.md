# Development log

Newest entries first. Each entry records what was implemented, the tests
that cover it, and decisions worth remembering. Future contributors (human
or agent): read `docs/architecture.md` first, then the latest entries here,
then `docs/roadmap.md` for what to build next.

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
