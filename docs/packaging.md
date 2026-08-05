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

Target artifacts are coordinated by `.github/workflows/release.yml` on `v*`
tags and on pushes to `main` for the rolling continuous prerelease. Its Linux
job calls the reusable `.github/workflows/appimage.yml` builder; the same
builder automatically runs on relevant branch pushes, including `main`, and
can be dispatched manually when only a test AppImage is needed.

| Artifact | Tooling | Status |
|---|---|---|
| Linux AppImage | linuxdeploy + Qt plugin | **implemented** (`release-build-linux`) |
| Windows portable zip | `windeployqt` into a folder, zip it | **implemented** (`release-build-windows`) |
| Windows installer | NSIS over the portable tree | **implemented** (`release-build-windows`) |

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
On `main` pushes, `publish-continuous` updates the rolling prerelease at
`https://github.com/nexdep/syodep/releases/tag/continuous` with
`syodep-continuous-win64.zip` and `syodep-continuous-x86_64.AppImage`.
Manual `release.yml` runs stop at workflow artifacts.

### Linux AppImage (implemented)

The reusable AppImage builder runs in an **`ubuntu:22.04` container** on the
24.04 runner: an AppImage inherits the glibc floor of its build machine, and
22.04's glibc 2.35 covers Ubuntu 22.04+, Debian 12+, Fedora 36+ and anything
newer. Qt (6.2 LTS) comes from the container's apt and is bundled; Rust is
installed via rustup inside the container. Cargo dependencies and their native
build outputs are cached across runs, but workspace crates are rebuilt so the
binary always carries the selected commit's identity.

Packaging uses `linuxdeploy` + `linuxdeploy-plugin-qt` (prebuilt
binaries, run with `--appimage-extract-and-run` since containers lack
FUSE) with `packaging/syodep.desktop` and `packaging/syodep.svg`.
Bundled: the binary, Qt libs, platform plugins (xcb, wayland, plus
`offscreen` via `EXTRA_PLATFORM_PLUGINS` for headless/smoke-test use), and
Wayland's separate
`wayland-graphics-integration-client/libqt-plugin-wayland-egl.so`. The latter
is required for a Wayland `QOpenGLWidget`; a top-level
`platforms/libqwayland-egl.so` alone can load while leaving Qt with no client
buffer integration. `qt6-wayland` is installed in the build container so all
of these plugins come from the same Qt 6.2.4 installation. The workflow
extracts the finished AppImage, asserts the exact plugin and its
`libQt6WaylandEglClientHwIntegration.so.6` dependency are present, and rejects
unresolved dynamic-library dependencies.
Excluded by linuxdeploy's default list and resolved from the host:
glibc, libGL, fontconfig — exactly the libs that must match the user's
system.

The job then smoke-tests the actual AppImage twice with a generated PDF. Xvfb
simulates WSL without a Wayland socket and must select XCB; headless Weston
simulates WSLg and must select Wayland. Both paths construct the real
`QOpenGLWidget` canvas and require a valid OpenGL context, so an incomplete
bundle fails in CI before `syodep-x86_64.AppImage` is uploaded.

For a Linux-only development build, push a branch change under `crates/`,
`ui-qt/`, `packaging/`, or `.github/workflows/`; **AppImage Preview** runs
automatically. On `main`, it intentionally builds alongside the release
workflow so its downloadable artifact is ready as soon as the Linux job
finishes, without waiting for the Windows build and `continuous` publication.
You can also open **Actions → AppImage Preview → Run workflow**, select any
branch to build, and download the
`syodep-x86_64-appimage` artifact when the run finishes. The preview is not a
reduced package: it uses the exact release builder and both smoke tests. The
artifact is retained for 14 days and does not create or update a GitHub
release. The equivalent CLI flow is:

```bash
gh workflow run appimage.yml --ref <branch>
gh run watch
gh run download <run-id> -n syodep-x86_64-appimage
```

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
`scoop update syodep` always finds the newest asset. The continuous
prerelease does not update Scoop metadata. The manifest commit comes from
`github-actions[bot]`.

### Windows installer (implemented)

`packaging/syodep.nsi`, compiled by `makensis` in `release-build-windows` over
the same `syodep-win64/` tree the staged smoke test has already validated, and
attached to tagged releases as `syodep-vX.Y.Z-win64-setup.exe`. The rolling
`continuous` prerelease carries only the zip and the AppImage: an installer that
writes registry entries is a poor fit for a build that changes on every merge.

- **Per-user.** Installs to `%LOCALAPPDATA%\Programs\syodep`, no UAC prompt,
  works without administrator rights. This is also what lets CI verify a real
  install/uninstall round trip, which an elevated installer could not do.
- **Silent capable.** `/S` installs without UI; `/D=<dir>` overrides the
  location and **must be the last argument, unquoted, with no trailing
  backslash**. `/ASSOCIATE` opts into the PDF registration. `uninstall.exe /S`
  uninstalls, and the Add/Remove Programs entry publishes a
  `QuietUninstallString` for winget/MDM tooling.
- **The PDF association is opt-in and cannot claim the default.** Since
  Windows 8 the effective handler lives in a hash-protected `UserChoice` key no
  installer can forge. The checkbox registers a ProgID plus
  `Applications\syodep.exe` and `OpenWithProgids`, which makes syodep *appear*
  in "Open with" and in Settings → Default apps, where the user confirms.
  The "Open with" registration happens unconditionally; only the picker entry
  is gated on the checkbox.
- **Uninstall leaves `%APPDATA%\syodep` alone.** Config and reading positions
  are shared with Scoop and portable installs, so deleting them would destroy
  state this installer never owned.
- **Unsigned.** SmartScreen will show "Windows protected your PC" until the
  binary earns reputation, and reputation is per-publisher so an unsigned build
  can never accrue any. Users who would rather avoid the prompt should install
  via Scoop, which downloads the zip programmatically.

The script is syntax-checked on **Linux** in the `rust-lint` CI job -- `makensis`
is cross-platform, so a broken script fails in about a minute instead of after
the twelve-minute Windows build.

## Versioning

Workspace version lives in `Cargo.toml` (`workspace.package.version`) and is
mirrored in the top-level `project(syodep VERSION …)`, which in turn defines
`SYODEP_VERSION` for the Qt shell (`ui-qt/CMakeLists.txt`). The shell reports it
through `QApplication::setApplicationVersion`, so `--version` and `--check`
cannot disagree with the core they link against. **`Cargo.toml` is the only
place to edit when bumping**; `scripts/check-docs.sh` fails if CMake drifts from
it, or if the shell reintroduces a hardcoded version string.

That check exists because the mirror was previously only a claim: CMake sat at
`0.3.0` through the whole 0.4.0 release and the shell hardcoded `0.3.0` too, so
shipped 0.4.0 binaries reported `syodep 0.3.0` while their own core reported
`0.4.0`. Nothing caught it.

Tags use `vX.Y.Z`. The non-version `continuous` tag is force-updated by CI to
point at the latest successful `main` build and must not be treated as a
semantic version.
