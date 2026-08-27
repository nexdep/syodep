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
builder automatically runs on relevant branch pushes, excluding `main`, and
can be dispatched manually when only a test AppImage is needed.

| Artifact | Tooling | Status |
|---|---|---|
| Linux AppImage | linuxdeploy + Qt plugin | **implemented** (`release-build-linux`) |
| Windows portable zip | `windeployqt` into a folder, zip it | **implemented** (`release-build-windows`) |
| Windows installer | NSIS over the portable tree | **implemented** (`release-build-windows`) |

### Windows (implemented)

CI (`qt-build-windows` in `ci.yml`) builds the Qt shell on every push/PR:
Qt 6.7.3 via `jurplel/install-qt-action` (`win64_msvc2019_64`), MSVC
environment via `step-security/msvc-dev-cmd`, `cmake -G Ninja`, then the offscreen
smoke test. The exe is a GUI-subsystem binary, so the smoke test is judged
by exit code (stdout is invisible on Windows).

The release job (`release-build-windows` in `release.yml`) additionally:

1. builds in Release mode,
2. stages `syodep-win64/` with `windeployqt --release --no-translations`
   plus `LICENSE`, `README.md` and the sample config,
3. re-runs the smoke test from the staged tree **with Qt stripped from
   PATH**, so an incomplete DLL set fails in CI rather than on a user's
   machine,
4. zips the portable tree, builds the NSIS installer, and exercises a real
   install/uninstall cycle, and
5. uploads `syodep-win64.zip` as a workflow artifact; tag and manual runs also
   include `syodep-setup.exe`, while `main` discards the installer after its
   tests because the continuous publisher consumes only the zip.

On `v*` tag pushes a final job (`publish-release`) creates a GitHub
release and attaches the zip as `syodep-vX.Y.Z-win64.zip` and the
AppImage as `syodep-vX.Y.Z-x86_64.AppImage`, with generated notes. That job
waits on both builds, because a versioned release has to be complete.

On `main` pushes the rolling prerelease at
`https://github.com/nexdep/syodep/releases/tag/continuous` is updated by two
independent jobs, one per platform, neither waiting on the other — see
[Rolling continuous release](#rolling-continuous-release-implemented).
Manual `release.yml` runs stop at workflow artifacts.

### Workflow artifact retention

Workflow artifacts are short-lived transfer and debugging objects, not the
permanent user-facing downloads attached to GitHub Releases. Artifacts produced
by a `main` push are retained for 3 days. Branch, tag, and manual-run artifacts
are retained for 7 days. This applies to the post-validation native Linux tar,
the AppImage passed to its publisher, and `syodep-win64`; every upload declares
its retention explicitly instead of inheriting the repository default.

The 3-day native tar preserves the policy that every push leaves a CI artifact
after all required checks pass. The AppImage and Windows zip from `main` need
only survive long enough for their independent continuous publishers to consume
them; successful user-facing copies remain on the rolling prerelease. Versioned
release assets are likewise permanent independently of their 7-day transfer
artifacts.

### Linux AppImage (implemented)

The reusable AppImage builder runs in an **`ubuntu:24.04` container** on the
24.04 runner. An AppImage inherits the glibc floor of its build userland, so the
published package now requires glibc 2.39 (Ubuntu 24.04 or another distribution
with an equivalent runtime). Qt comes from the container's apt and is bundled;
Rust is installed via rustup inside the container. Cargo dependencies and their
native build outputs are cached across runs under an Ubuntu-24.04-specific key,
while workspace crates are rebuilt so the binary always carries the selected
commit's identity.

Packaging uses `linuxdeploy` + `linuxdeploy-plugin-qt` (prebuilt
binaries, run with `--appimage-extract-and-run` since containers lack
FUSE) with `packaging/syodep.desktop` and `packaging/syodep.svg`.
Bundled: the binary, Qt libs, only the generic Wayland and Wayland-EGL QPA
plugins, and Wayland's separate
`wayland-graphics-integration-client/libqt-plugin-wayland-egl.so`. The latter
is required for a Wayland `QOpenGLWidget`; a top-level
`platforms/libqwayland-egl.so` alone can load while leaving Qt with no client
buffer integration. Because linuxdeploy assigns that manually seeded nested
plugin a RUNPATH relative to the AppDir root, packaging corrects it to
`$ORIGIN/../../lib:$ORIGIN` after deployment. The bundle check requires its
`libQt6WaylandEglClientHwIntegration.so.6` dependency to resolve specifically
from the AppDir's `usr/lib`, so a copy installed on the build host cannot mask a
broken AppImage. The Qt deployment plugin adds XCB by default, so the job
populates `AppDir` first, deletes every QPA plugin except the two Wayland ones,
and only then creates the AppImage. `qt6-wayland` is installed in the build
container so all of these plugins come from the same distro Qt installation.
The workflow extracts the finished AppImage, asserts XCB/offscreen are absent,
checks the exact integration plugin and its
`libQt6WaylandEglClientHwIntegration.so.6` dependency are present, and rejects
unresolved dynamic-library dependencies.
Excluded from the bundle and resolved from the host: glibc, libGL, fontconfig,
and `libxkbcommon.so.0` — libraries that integrate with system drivers, fonts,
or locale data. In particular, xkbcommon parses the host's X11 Compose table;
bundling the build container's parser while reading a different host table can
emit keysym errors and break only the affected compose sequences. Packaging
removes both xkbcommon and its X11 companion even if the Qt deploy plugin copied
them, then checks the extracted AppImage still resolves xkbcommon from the host.
Supported Wayland systems must therefore provide the stable
`libxkbcommon.so.0` ABI in addition to the graphics libraries.

The job starts headless Weston and smoke-tests the actual AppImage with both
`--renderer=opengl` (Mesa software GL on the GPU-less runner) and
`--renderer=raster`. It also checks that `auto` selects OpenGL there and that
an XCB override is rejected before Qt starts. An incomplete bundle or a
backend-specific paint failure therefore fails before upload.

The Linux artifact intentionally cannot run on an Xorg-only desktop or through
X11-only remote display. Raster is an OpenGL fallback, not a Wayland fallback:
no compositor/socket, an unloadable Qt Wayland plugin, or an unusable Wayland
shared-memory backing store still prevents startup. A forced OpenGL renderer
also exits when the one-frame probe fails; `auto` and `raster` remain usable in
that case when the Wayland raster path works.

For a Linux-only development build, push a **feature branch** change under
`crates/`, `ui-qt/`, `packaging/`, `scripts/`, or `.github/workflows/`;
**AppImage Build** runs automatically and leaves a downloadable workflow
artifact. You can also open **Actions → AppImage Build → Run workflow**, select
any branch, and download the `syodep-x86_64-appimage` artifact when the run
finishes. This is not a reduced package: it uses the exact release builder and
both smoke tests. It does not create or update a GitHub release, and branch and
manual artifacts are retained for 7 days. The equivalent CLI flow is:

```bash
gh workflow run appimage.yml --ref <branch>
gh run watch
gh run download <run-id> -n syodep-x86_64-appimage
```

`main` is deliberately excluded from that trigger. It reaches the same builder
through `release.yml`, which publishes the AppImage from that build rather than
running a second, byte-identical one.

### Rolling continuous release (implemented)

Every push to `main` refreshes the prerelease at
`https://github.com/nexdep/syodep/releases/tag/continuous`, through two jobs
that are siblings rather than a chain:

| Job | Waits on | Publishes | Typical latency |
|---|---|---|---|
| `publish-continuous-linux` | Linux build only | `syodep-continuous-x86_64.AppImage` | ~3 min |
| `publish-continuous-windows` | Windows build only | `syodep-continuous-win64.zip`, Scoop bump | ~8.5 min |

Both live in `release.yml` rather than inside the reusable AppImage workflow.
That placement is deliberate: a job inside the reusable workflow contributes to
the caller's `uses:` job result, so a failing publisher fails
`release-build-linux` and takes the whole continuous release down with it —
observed on `a15d15f`, where a publisher that could not get a runner skipped the
continuous release entirely. Neither half may be able to block the other.

The Release workflow has a workflow-level concurrency group keyed by workflow,
event and ref. A newer `main` push cancels the older run before its builders
spend more runner time, while tags, feature branches and manual builds are not
auto-cancelled. Each publisher also carries its own per-platform concurrency
group. Sharing that publisher group would have Linux and Windows cancel each
other, since both set `cancel-in-progress`.

Both call `scripts/ensure-continuous-release.sh` to move the `continuous` tag
and create-or-update the release before uploading. That script has to be
idempotent and safe to run concurrently: on a repository with no `continuous`
release yet both callers race to create it, so it tolerates losing that race and
verifies the result rather than trusting its own create.

#### The two assets can be from different commits

Publishing each platform as soon as it is ready means the release is no longer
atomic across platforms. Between the AppImage upload (~3 min) and the zip upload
(~8.5 min) the release holds a Linux build from this push and a Windows build
from the previous one. With pushes closer together than the Windows build takes,
the zip can lag further: its freshness check skips any run whose commit is no
longer `origin/main`, so it stays put until a run survives to the end.

This is the deliberate trade for latency. Previously the two assets always
matched each other, but a superseded run skipped the whole job and *both* went
stale together; now Linux is always as current as possible and Windows may trail
it. The release notes say so, and every binary reports the commit it was built
from via `syodep --version`, so an asset's provenance is never ambiguous.

### Scoop (implemented)

The repo doubles as a Scoop bucket, with one manifest per channel. Both point
at the Windows zip (with `extract_dir`, `bin`, a Start Menu shortcut) and both
are bumped by CI, committed to `main` by `github-actions[bot]`.

The repository must remain public. Scoop clones this repository as a bucket and
downloads its GitHub release assets without repository credentials; making the
repository private prevents bucket refreshes and installs even when the
manifests and assets still exist.

```powershell
scoop bucket add syodep https://github.com/nexdep/syodep
scoop install syodep              # tagged releases
scoop install syodep-continuous   # rolling build from main
```

The two can be installed side by side: the continuous manifest shims
`syodep-continuous` and names its shortcut "syodep (continuous)", so neither
overwrites the other. They still share `%APPDATA%\syodep`, as every other
Windows install method does.

**`bucket/syodep.json`** — bumped by `publish-release` after every tag release.
`jq` rewrites `version`/`url`/`hash`; the URL moves because each release gets
its own versioned asset. Carries `checkver`/`autoupdate` metadata so Scoop's
own tooling can also spot a new release.

**`bucket/syodep-continuous.json`** — bumped by `publish-continuous-windows`
after every push to `main`. Only `version`/`hash` move: the download URL is fixed,
because the `continuous` release assets are clobbered in place. That is also
why the version has to change on every build — Scoop re-downloads on a version
change, not on a moved hash, so a static version would pin every user to
whatever zip they first fetched. The version is
`<base>-continuous.<YYYYMMDD>.<HHMMSS>+<sha12>`; date and time are separate
components because Scoop compares numeric version parts as 32-bit integers and
a single `YYYYMMDDHHMMSS` stamp overflows that. No `checkver`/`autoupdate`:
that timestamp is minted by CI and no regex over the releases API can
reconstruct it.

Both bump commits carry **`[skip ci]`**: CI's own manifest commits never
trigger CI.

For `publish-continuous-windows` the marker is load-bearing. The commit lands
on `main`, and a `main` push is exactly what triggers that job — without it it
would publish, bump, push, and re-trigger itself forever.

For `publish-release` it is only economy. That push used to start a full
Linux + Windows + installer run over a one-line JSON change, and the rolling
`continuous` prerelease it rebuilt differed from the previous one by nothing
else. The cost of marking it: after a tagged release, `continuous` keeps
pointing at the commit before the manifest bump until the next real merge —
a JSON-only difference in a prerelease that changes on every merge anyway.

`main` can also move between a job's checkout and its push, which git rejects.
Both bumps therefore retry up to three times, rebasing onto the newer `main`.
The rebase is safe in either case: nothing else edits those files, and a
manifest describes the assets its own run published, not the state of the tree
it lands on. Both jobs check out with `fetch-depth: 0`, because the default
depth-1 clone has no merge base to rebase onto.

The race is much likelier for `publish-continuous-windows`, which runs on every
merge,
but losing the push matters more for `publish-release`: a dropped continuous
bump is repaired by the next merge's run, whereas a dropped release bump leaves
`scoop install syodep` on the previous version until someone notices.

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

The script is syntax-checked on **Linux** in the `Linux validation` CI job --
`makensis` is cross-platform, so a broken script fails before the longer
Windows release build completes.

## Versioning

Workspace base version lives in `Cargo.toml` (`workspace.package.version`).
**That remains the only place to edit when bumping.** CMake combines that base
with a build channel and the first 12 characters of the Git commit:

| Build | Example reported by `--version` / `--check` |
|---|---|
| regular version tag `v0.16.0` | `0.16.0` |
| versioned prerelease tag `v0.16.0-rc.1` | `0.16.0-rc.1` |
| rolling `continuous` release, both assets | `0.16.0-continuous+012345abcdef` |
| feature-branch or manual AppImage build, or a local checkout | `0.16.0-dev+012345abcdef` |

If the Cargo base is already a prerelease, a non-release channel extends it:
`0.16.0-rc.1.continuous+012345abcdef`. Thus every identity remains valid
SemVer and a rolling or development binary cannot claim to be the regular
release.

`SYODEP_BUILD_CHANNEL` accepts `auto`, `release`, `continuous` and
`development`. It is `auto` for ordinary builds: a clean checkout exactly at
`v<base-version>` resolves to `release`; everything else resolves to
`development`. Packaging workflows pass `release`, `continuous`, or
`development` explicitly, so a build's channel never depends on how the runner
happened to check the tree out. CMake writes the result to `build/syodep-version.txt`, defines it
for the Qt shell and Windows version-resource strings, and injects the same
value into the Rust build for `syo_core_version`. The installer reads that
generated file too, so its DisplayVersion agrees with its binary.

The shell reports the identity through `QApplication::applicationVersion`.
The core returns the injected value over the FFI, so the `shell:` and `core:`
lines cannot disagree. Plain Cargo-only builds have no distribution channel
and fall back to the Cargo base version. `scripts/check-docs.sh` guards this
construction against a hardcoded version returning. The separate `build type`
line means the CMake optimization configuration (`Release`/`Debug`), not the
distribution channel.

That check exists because the mirror was previously only a claim: CMake sat at
`0.3.0` through the whole 0.4.0 release and the shell hardcoded `0.3.0` too, so
shipped 0.4.0 binaries reported `syodep 0.3.0` while their own core reported
`0.4.0`. Nothing caught it.

Tags use `vX.Y.Z` or a Cargo-compatible prerelease such as `vX.Y.Z-rc.1`. The
non-version `continuous` tag is force-updated by CI to point at the newest
rolling build and must not be treated as a semantic version.
