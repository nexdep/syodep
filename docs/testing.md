# Testing

## Strategy

The architecture is chosen for testability: nearly all behavior lives in
pure Rust crates that run without a display, a real window, or (mostly)
real files. The strategy, in descending order of coverage:

1. **TDD unit tests** for all pure core logic — key parsing, the input
   state machine (counts, sequences, disambiguation, Escape), layout math
   (stacking, clamping, zoom anchoring, visible pages), the render cache
   (hits, LRU eviction, error propagation), config parsing and its error
   messages, migrations.
2. **Integration tests** at the `App` level: open → navigate via key
   sequences → persist → reopen → position restored; rename-resilient
   persistence; failure paths (missing file, invalid bindings).
3. **FFI round-trip tests** exercising the C ABI exactly as the Qt shell
   does (including NULL-argument tolerance).
4. **PDF smoke tests** against *generated* fixtures: a programmatic,
   spec-conforming PDF builder (`syodep-pdf/src/test_support.rs`, feature
   `test-support`) creates multi-page documents with known text, so no
   binary fixtures live in the repository. Alongside `pdf_with_pages` there
   are `pdf_two_column_page`, `pdf_with_image`, `pdf_with_table` (a ruled
   grid between a heading and a caption, for table detection) and
   `pdf_with_heading` (a large heading plus a bold subheading over body prose),
   `pdf_with_running_header` (multi-page, with the header either constant or
   varying, plus a folio), `pdf_with_rotated_text` (a sideways stamp and an
   inclined watermark, or a page laid out entirely sideways), `pdf_with_list`
   (a colon lead-in, three bulleted items, a closing sentence),
   `pdf_with_table_gap` (a table with its caption held back by a configurable
   gap, for the edge-trimming rule) and `pdf_with_equation` (a display formula
   set apart from body prose, for equation detection).

   Navigation over tables is tested two ways on purpose. The mapping from
   detected boxes to line ranges (`content_objects`) is a pure function tested
   directly, and the motion tests inject a hand-built `PageContent` rather than
   relying on MuPDF's heuristic — so a change in that heuristic can only ever
   fail the one detection test, never the behavioural suite.
5. **Shell smoke test** in CI: `syodep --smoke-test file.pdf` with
   `QT_QPA_PLATFORM=offscreen` constructs the real window, opens a document
   through the FFI, renders a page and paints one frame.

What is intentionally *not* unit-tested: Qt widget behavior (kept so thin
that the smoke test plus compilation covers it) and MuPDF internals (we
test our wrapper's contract: open, sizes, render dimensions/format, text).

Regression rule: every bug fix lands together with a test that fails
before the fix.

## Running

```bash
# everything (needs no Qt, no display)
cargo test --workspace

# single crate
cargo test -p syodep-core

# lints exactly as CI runs them
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

# shell smoke test (after a CMake build)
cargo run -p syodep-pdf --features test-support --example make_fixture -- /tmp/f.pdf 5
QT_QPA_PLATFORM=offscreen ./build/ui-qt/syodep --smoke-test /tmp/f.pdf

# docs consistency (commands/keybindings/config all documented)
./scripts/check-docs.sh
```

## CI

`.github/workflows/ci.yml` runs on every push and pull request:

| Job | Contents |
|---|---|
| `rust-lint` | `cargo fmt --check`, `clippy -D warnings` |
| `rust-test-linux` | full `cargo test --workspace` (config, storage/migrations, core, pdf, ffi) |
| `rust-test-windows` | same on Windows (MSVC) |
| `qt-build-linux` | CMake configure + build of the Qt shell, then the offscreen smoke test |
| `qt-build-windows` | same on Windows (Qt via aqtinstall, MSVC + Ninja); smoke test judged by exit code (GUI-subsystem exe has no stdout) |
| `docs` | `scripts/check-docs.sh`: required docs exist; every command, default keybinding and config option is documented |
| `build-artifact` | packages the build as a downloadable CI artifact on every push (see `docs/packaging.md`) |

The release workflow additionally smoke-tests the staged Windows portable
tree with Qt removed from PATH, catching missing bundled DLLs
(see `docs/packaging.md`).

Release pipeline: see `docs/packaging.md`.

## Current coverage snapshot

452 Rust tests: 30 config, 275 core, 119 pdf, 15 storage, 13 ffi — plus the CI
smoke test and docs checks.
