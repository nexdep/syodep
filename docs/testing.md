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
   binary fixtures live in the repository.
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
| `docs` | `scripts/check-docs.sh`: required docs exist; every command, default keybinding and config option is documented |

Release pipeline: see `docs/packaging.md`.

## Current coverage snapshot (milestone 1)

88 Rust tests: 15 config, 48 core, 12 pdf, 9 storage, 4 ffi — plus the CI
smoke test and docs checks.
