# AGENTS.md

Guidance for agents working on syodep. Read `docs/architecture.md` first,
then the latest entries in `docs/development-log.md`, then `docs/roadmap.md`
for what to build next.

## Hard rules

- **Core/UI split**: the Rust core (`crates/`) must never depend on Qt; the
  Qt shell (`ui-qt/`) must contain no document/navigation logic. The shell
  forwards events through the C ABI (`crates/syodep-ffi`) and paints what
  the core returns. New behavior goes in the core as a `Command`, never in
  Qt event handlers.
- **Clean-room policy**: [Sioyek](https://github.com/ahrm/sioyek) is
  product inspiration only. A local clone *may* exist at
  `../sioyek-reference` (a sibling of this repo, not part of it); if it
  does, treat it as read-only — never copy code, file structure, or assets
  from it (clean-room policy by project decision, regardless of license
  compatibility), and never make syodep depend on it. If it is absent,
  simply work without it. Record borrowed *ideas* in the "Sioyek:
  conceptual inspirations" section of `docs/architecture.md`.
- **Unsafe stays at the edges**: MuPDF only inside `crates/syodep-pdf`
  (no MuPDF types may leak from it); raw pointers only inside
  `crates/syodep-ffi` (every entry point wrapped in `catch_unwind`).
- **Persistence**: dynamic user state goes in SQLite, never TOML.
  `MIGRATIONS` in `crates/syodep-storage/src/migrations.rs` is append-only;
  never edit a published entry. An entry that has not been in a release is not
  published: if the feature it belongs to is abandoned, delete the entry rather
  than adding a second one to undo it, and say in the dev log that development
  databases have to be recreated (this happened once, to v4).
- **Config errors never abort the app**: degrade to defaults + status-bar
  warning.

## Definition of done (enforced by CI)

A feature is complete only when implemented, tested, and documented:
commands in the per-mode pages indexed by `docs/commands.md`
(`docs/commands-normal-mode.md`, `docs/commands-focus-mode.md`,
`docs/commands-visual-mode.md`, `docs/commands-highlight-mode.md`),
default bindings in `docs/keybindings.md`,
config options in `docs/config.md` (the `docs` CI job greps source registries
against these files — `scripts/check-docs.sh`), plus a dev-log entry for the
milestone. TDD for core logic; if TDD is impractical, state the test
strategy first. Bug fixes land with a regression test.

## Build & verify

```bash
cargo test --workspace                                # no Qt/display needed
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/check-docs.sh

# Qt shell (cmake drives cargo for the syodep-ffi staticlib):
cmake -B build -G Ninja && cmake --build build
cargo run -p syodep-pdf --features test-support --example make_fixture -- /tmp/f.pdf 5
bash scripts/with-headless-wayland.sh \
  ./build/ui-qt/syodep --renderer=opengl --smoke-test /tmp/f.pdf
bash scripts/with-headless-wayland.sh \
  ./build/ui-qt/syodep --renderer=raster --smoke-test /tmp/f.pdf
```

## Build artifacts & releases

Every push must produce a CI build artifact after the full test/docs/lint/smoke
suite passes. Branch pushes produce ephemeral artifacts named with the commit
SHA. Main pushes also update the rolling `continuous` prerelease with the
latest AppImage and Windows zip; the two assets are published by separate jobs
as soon as each platform finishes building, so they can be from different
commits. Versioned public releases are created by
`.github/workflows/release.yml` only from `vMAJOR.MINOR.PATCH` tags.

Do not auto-commit version bumps from CI. Development builds derive their
identity from git metadata. Do not check generated binaries into the repo.
Changes to packaging, release workflows, or version metadata must update the
dev log and keep `.github/workflows/ci.yml` green.

**Before pushing any commit**: all of the above must pass — the full test
suite green, lints clean, and documentation consistent with the code
(`./scripts/check-docs.sh` plus an updated dev log when behavior changed).
Never push red. CI mirrors these checks in `.github/workflows/ci.yml`
(plus Windows `cargo test`).

**Before pushing to `main`** (directly or via merge): always verify the
documentation is consistent with the code before the push — this is a hard
gate, never skipped. Run `./scripts/check-docs.sh` and confirm it passes,
and additionally review by hand that the docs indexed under "Definition of
done" still match what the code does:

- per-mode command pages (`docs/commands-normal-mode.md`,
  `docs/commands-focus-mode.md`, `docs/commands-visual-mode.md`,
  `docs/commands-highlight-mode.md`) match `Command`/`ALL_COMMANDS`,
- `docs/keybindings.md` matches `syodep-config::default_keybindings()`,
- `docs/config.md` matches the config registry,
- `docs/architecture.md` and `docs/roadmap.md` still reflect current
  behavior, and a dev-log entry covers any behavior change in the push.

If any doc is stale, update it (or the code) so they agree before pushing.
Do not push to `main` with inconsistent documentation.

## Conventions & gotchas

- Adding a command: extend `Command` + `ALL_COMMANDS` in
  `crates/syodep-core/src/command.rs`, handle it in `App::execute`, add a
  default binding in `syodep-config::default_keybindings()` if warranted,
  document all of it. Counts come free via the input state machine.
- Key syntax (`gg`, `<C-d>`) is parsed in `syodep-config::keys` and produced
  by `ui-qt/src/key_encoder.cpp`; keep the two in sync if extending it.
- A sequence that is a prefix of another binding waits for the next key press
  or a pause. It still fires: if the longer sequence turns out to be unbound,
  the longest bound prefix runs and the leftover keys are replayed (`oj` → `o`
  then `j`). Because the replay can change mode, it is drained by
  `App::dispatch`, not inside `InputState`.
- The pause lives in the **shell**: `InputState::timeout` is called by a QTimer
  the widget arms when `Effects::pending_input` is set. The core never reads a
  clock, so it stays deterministic and a test "waits" by calling `timeout`.
- Coordinates: document space = PDF points, zoom-independent; scroll state
  is stored in document space. Screen = `(doc - scroll) * zoom`, physical
  pixels (the shell multiplies by devicePixelRatio).
- FFI: returned strings/bitmaps are owned by the caller — pair every new
  allocation with a `syo_*_free`; header is cbindgen-generated into
  `crates/syodep-ffi/include/` (gitignored, do not hand-edit).
- PDF test fixtures are generated by `syodep_pdf::test_support::pdf_with_pages`
  (feature `test-support`); never check in binary PDFs.
- MuPDF `Document` is `!Send`; rendering is synchronous on the UI thread
  for now (async tiles are a phase-3 roadmap item behind `App::render_page`).
- CMake builds Rust with the `release` profile even for Debug C++
  (`-DSYODEP_RUST_PROFILE=dev` to override); Linux link needs
  `fontconfig`/`freetype`.
- First Rust build compiles vendored MuPDF (~5 min); needs clang/libclang.
