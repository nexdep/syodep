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
discovery); Windows links `ws2_32 userenv bcrypt ntdll advapi32 secur32
ncrypt` for the Rust runtime/SQLite.

## Release pipeline (specification)

Target artifacts, produced by `.github/workflows/release.yml` on `v*` tags:

| Artifact | Tooling | Status |
|---|---|---|
| Linux AppImage | linuxdeploy + Qt plugin | **planned** (placeholder job exists) |
| Windows portable zip | `windeployqt` into a folder, zip it | **planned** |
| Windows installer | NSIS over the portable tree | **planned** |

The current `release.yml` is an intentional placeholder: it validates a
clean release-mode build on tag pushes and uploads the raw Linux binary,
so the packaging steps land on a known-good foundation. Implementing the
real packaging is a roadmap item (see `docs/roadmap.md`, "Packaging
milestone").

Planned specifics:

- **AppImage:** build on the oldest supported LTS runner for glibc
  compatibility; bundle Qt platform plugins (xcb, wayland) via linuxdeploy's
  Qt plugin; desktop file + icon; output `syodep-x86_64.AppImage`.
- **Windows:** install Qt via `aqtinstall` (pinned version) on
  `windows-2022`; `cmake --build` with MSVC; `windeployqt --release
  --no-translations` into `syodep-win64/`; zip for the portable artifact;
  NSIS script (silent-install capable) for the installer.
- Both jobs end with the offscreen smoke test against the packaged binary
  before uploading.

## Versioning

Workspace version lives in `Cargo.toml` (`workspace.package.version`) and
is mirrored in the top-level `project(syodep VERSION …)`. Tags use `vX.Y.Z`.
