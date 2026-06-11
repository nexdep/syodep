# Packaging and release pipeline

## Build system

Top-level CMake orchestrates both halves:

1. A custom target runs `cargo build -p syodep-ffi` producing a static
   library (`libsyodep_ffi.a` / `syodep_ffi.lib`) plus the cbindgen header.
   MuPDF and SQLite are compiled from vendored sources by cargo — no system
   packages needed for them on either platform.
2. The Qt shell (`ui-qt/`) links Qt6::Widgets, Qt6::OpenGLWidgets and the
   static core.

The Rust profile for the core defaults to `release` even in Debug C++
builds (`-DSYODEP_RUST_PROFILE=dev` to override) because debug MuPDF
rendering is unusably slow.

Platform link extras: Linux needs `fontconfig`/`freetype` (system font
discovery). On Windows the exact system-library set is queried from
`rustc --print=native-static-libs` at configure time, and MuPDF's static
libs (not bundled into the staticlib there, unlike Linux) are resolved to
their cargo `OUT_DIR` paths. The Windows shell must be built as Release:
a debug config links `/MDd` against MuPDF's `/MD` objects and fails.

## Release pipeline (specification)

Target artifacts, produced by `.github/workflows/release.yml` on `v*` tags:

| Artifact | Tooling | Status |
|---|---|---|
| Linux AppImage | linuxdeploy + Qt plugin | **planned** (placeholder job exists) |
| Windows portable zip | `windeployqt` into a folder, zip it | **implemented** (`release-build-windows`) |
| Windows installer | NSIS over the portable tree | **planned** |

### Windows (implemented)

CI (`qt-build-windows` in `ci.yml`) builds the Qt shell on every push/PR:
Qt 6.7.3 via `jurplel/install-qt-action` (`win64_msvc2019_64`), MSVC
environment via `ilammy/msvc-dev-cmd`, `cmake -G Ninja`, then the offscreen
smoke test. The exe is a GUI-subsystem binary, so the smoke test is judged
by exit code (stdout is invisible on Windows).

The release job (`release-build-windows` in `release.yml`) additionally:

1. builds in Release mode,
2. stages `syodep-win64/` with `windeployqt --release --no-translations`
   plus `LICENSE`, `README.md` and the sample config,
3. re-runs the smoke test from the staged tree **with Qt stripped from
   PATH**, so an incomplete DLL set fails in CI rather than on a user's
   machine,
4. zips and uploads `syodep-win64.zip` as a workflow artifact.

On `v*` tag pushes a final job (`publish-release`) creates a GitHub
release and attaches the zip as `syodep-vX.Y.Z-win64.zip` with generated
notes. Manual (`workflow_dispatch`) runs stop at workflow artifacts.

### Still planned

- **Linux AppImage:** build on the oldest supported LTS runner for glibc
  compatibility; bundle Qt platform plugins (xcb, wayland) via linuxdeploy's
  Qt plugin; desktop file + icon; output `syodep-x86_64.AppImage`. The
  current Linux release job only validates a release-mode build and uploads
  the raw binary.
- **Windows NSIS installer:** silent-install capable script over the
  portable tree produced above.

## Versioning

Workspace version lives in `Cargo.toml` (`workspace.package.version`) and
is mirrored in the top-level `project(syodep VERSION …)`. Tags use `vX.Y.Z`.
