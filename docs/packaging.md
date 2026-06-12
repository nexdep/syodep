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
| Linux AppImage | linuxdeploy + Qt plugin | **implemented** (`release-build-linux`) |
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
release and attaches the zip as `syodep-vX.Y.Z-win64.zip` and the
AppImage as `syodep-vX.Y.Z-x86_64.AppImage`, with generated notes.
Manual (`workflow_dispatch`) runs stop at workflow artifacts.

### Linux AppImage (implemented)

`release-build-linux` runs in an **`ubuntu:22.04` container** on the
24.04 runner: an AppImage inherits the glibc floor of its build machine,
and 22.04's glibc 2.35 covers Ubuntu 22.04+, Debian 12+, Fedora 36+ and
anything newer. Qt (6.2 LTS) comes from the container's apt and is
bundled; Rust is installed via rustup inside the container.

Packaging uses `linuxdeploy` + `linuxdeploy-plugin-qt` (prebuilt
binaries, run with `--appimage-extract-and-run` since containers lack
FUSE) with `packaging/syodep.desktop` and `packaging/syodep.svg`.
Bundled: the binary, Qt libs, platform plugins (xcb, wayland, plus
`offscreen` via `EXTRA_PLATFORM_PLUGINS` for headless/smoke-test use).
Excluded by linuxdeploy's default list and resolved from the host:
glibc, libGL, fontconfig — exactly the libs that must match the user's
system.

The job then smoke-tests the actual AppImage (offscreen render of a
generated PDF), so an incomplete bundle fails in CI, and uploads
`syodep-x86_64.AppImage`.

### Scoop (implemented)

The repo doubles as a Scoop bucket: `bucket/syodep.json` points at the
release zip (with `extract_dir`, `bin`, a Start Menu shortcut, and
`checkver`/`autoupdate` metadata). Install:

```powershell
scoop bucket add syodep https://github.com/nexdep/syodep
scoop install syodep
```

The `publish-release` job rewrites the manifest's `version`/`url`/`hash`
(via `jq`) and commits the bump to `main` after every tag release, so
`scoop update syodep` always finds the newest asset. The manifest commit
comes from `github-actions[bot]`.

### Still planned

- **Windows NSIS installer:** silent-install capable script over the
  portable tree produced above.

## Versioning

Workspace version lives in `Cargo.toml` (`workspace.package.version`) and
is mirrored in the top-level `project(syodep VERSION …)`. Tags use `vX.Y.Z`.
