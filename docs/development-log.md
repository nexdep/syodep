# Development log

Newest entries first. Each entry records what was implemented, the tests
that cover it, and decisions worth remembering. Future contributors (human
or agent): read `docs/architecture.md` first, then the latest entries here,
then `docs/roadmap.md` for what to build next.

---

## 2026-08-26 — Move artifact transfers to Node 24

GitHub has begun retiring Node 20 for JavaScript actions. All workflow artifact
transfers now use `actions/upload-artifact@v6` and
`actions/download-artifact@v7`, the first corresponding majors that run on
Node 24 by default. This changes neither artifact names, paths, retention, nor
the conditional continuous-release publishing flow. GitHub-hosted runners meet
the actions' minimum runner version (2.327.1).

`scripts/check-docs.sh` now rejects a non-Node-24 artifact-action reference
and verifies that each upload/download path remains present. This makes a
future dependency downgrade fail the documentation CI check before it reaches
GitHub's deprecation deadline.

Test strategy: parse the edited workflow YAML, run the documentation check,
run the full local pre-push suite, then confirm the live CI and Release jobs
upload, download, and publish their artifacts without a Node 20 annotation.

## 2026-08-26 — Keep the PDF renderer clean on Rust 1.98

Rust 1.98 added the `chunks_exact_to_as_chunks` Clippy lint. The first live
verification of the Ubuntu 24.04 and retention changes exposed it because CI
tracks the current stable toolchain while the development machine was still on
Rust 1.97. Replaced the renderer's constant-size RGB/RGBA chunk iteration with
the equivalent fixed-array slice API; rendered bytes and application behavior
are unchanged.

Test strategy: the existing rendered-page test continues to verify the RGBA
buffer and visible ink, and the full workspace Clippy gate was reproduced with
Rust 1.98.0 and `-D warnings` before rerunning the complete pre-push suite.

## 2026-08-26 — Short-lived workflow artifacts

Workflow artifacts now have explicit retention instead of allowing the Windows
package to inherit the repository's 90-day default. Artifacts produced by
`main` live for 3 days; branch, tag, and manual-run artifacts live for 7. The
post-validation native Linux tar remains on `main` because every push is
required to leave an artifact after the full gate, but its lifetime is now
short. The AppImage and Windows zip are transfer objects whose durable copies
live on the rolling or versioned GitHub Release.

The Windows builder still compiles and exercises the NSIS installer on every
`main` push. Its main-only artifact contains just `syodep-win64.zip`, which is
all the continuous publisher consumes. Tag and manual runs retain both the zip
and `syodep-setup.exe` for 7 days, and tagged releases still attach the
installer permanently. `scripts/check-docs.sh` guards the retention expressions,
the two conditional Windows upload paths, and the packaging documentation.

Test strategy: parse the changed workflow YAML, exercise the documentation
consistency check, run the full pre-push Rust/lint/Qt smoke suite, then inspect
the real `main` workflow artifacts after both CI and Release finish. The
historical inventory was captured before removing superseded artifacts; one
newest artifact per known family and all release assets were preserved. The
before/after measurements are recorded in the main-push report rather than
hidden in CI.

## 2026-08-26 — Move the AppImage build baseline to Ubuntu 24.04

The reusable AppImage builder now uses an `ubuntu:24.04` container on its
Ubuntu 24.04 runner. This intentionally raises the published Linux package's
runtime floor from glibc 2.35 to glibc 2.39, so Ubuntu 22.04 is no longer a
supported AppImage host. The README, packaging specification, roadmap, release
workflow comments, CMake commentary, and main-push pipeline report now state
the same Ubuntu 24.04 baseline. `scripts/check-docs.sh` guards the workflow
container, cache namespace, glibc floor, and active documentation as one
invariant so a future baseline change cannot update only one of them.

The Rust cache shared key changed from `appimage-ubuntu-22.04` to
`appimage-ubuntu-24.04`. Native dependencies compiled against the former
userland must not be restored into the new build environment.

Test strategy: no core behavior changed. Validate the workflow structure with
the available local GitHub Actions/YAML tooling, run `scripts/check-docs.sh`,
and let the next AppImage workflow run exercise dependency installation,
packaging inspection, and the real OpenGL/raster AppImage smoke tests inside
the new container.

## 2026-08-10 — Give cold headless OpenGL startup enough time

The branch artifact job exposed a pre-existing race in
`scripts/with-headless-wayland.sh`: it allowed Weston exactly five seconds to
create its socket, while a cold GitHub runner took about 5.08 seconds to bring
up Mesa's llvmpipe renderer. Weston completed EGL and GL initialization, but the
helper reached its deadline just before the socket appeared. The ordinary Linux
Qt job passed because its earlier work had already warmed the same runner.

Raised the bounded startup wait from 5 to 15 seconds. The helper still exits as
soon as the socket appears and still fails immediately if Weston dies, so normal
runs gain no fixed delay and genuine startup failures remain fast.

Test strategy: shell syntax and both local headless OpenGL/raster smoke tests,
then a complete branch CI run whose separate `Build push artifact` job starts on
a cold runner. No core behavior changed.

## 2026-08-10 — Restore public repository visibility for distribution

The GitHub repository had been made private, which silently broke both public
distribution paths that use it directly. Existing Scoop users could not refresh
the `syodep` bucket, new users could not add it, and unauthenticated downloads
of the rolling `continuous` release were unavailable. A stale Windows bucket
therefore still contained only `syodep.json` and reported "Couldn't find
manifest for 'syodep-continuous'" even though the manifest existed on `main`.

Restored `nexdep/syodep` to public visibility. Public visibility is now recorded
as a packaging invariant alongside the Scoop bucket documentation: the bucket
and release assets are intentionally anonymous installation endpoints, not just
implementation details of CI.

Verification: GitHub reports `visibility: PUBLIC`; an unauthenticated Windows
`git ls-remote https://github.com/nexdep/syodep refs/heads/main` succeeds;
`scoop update` refreshes the bucket from one manifest to two, and
`scoop search syodep-continuous` resolves the rolling manifest from the
`syodep` bucket. No application source or package contents changed.

## 2026-08-07 — One rolling channel, published per platform

`main` pushes used to publish the Linux AppImage twice, from a single build, to
two releases: `appimage-preview` at ~3 min (Linux only) and `continuous` at
~8.5 min (both assets, gated on the Windows build). Both jobs downloaded the
same `syodep-x86_64-appimage` artifact and only renamed the file, so the two
AppImages were byte-identical. The split existed purely to give Linux a fast
download without waiting on Windows.

It also left the `appimage-preview` tag holding binaries built on the
`continuous` channel: `release.yml` picks the channel by ref, and the preview
publisher gated on the identical `push` + `main` condition, so a release named
"preview" never contained — and never could contain — a `-preview+<sha>` binary.
The entry below documented that mismatch rather than removing it.

Restructured so the axis is platform, not latency:

- `publish-continuous-linux` needs only the Linux build and uploads
  `syodep-continuous-x86_64.AppImage`.
- `publish-continuous-windows` needs only the Windows build, uploads
  `syodep-continuous-win64.zip`, and keeps the Scoop bump.
- `publish-appimage-preview` and the `appimage-preview` tag are gone. The
  AppImage now reaches `continuous` at the speed the preview used to, so the
  second release earned nothing.

The `preview` build channel goes with it. It only ever reached workflow
artifacts, and `development` already meant the same thing — "not a release, not
`main`". Branch and dispatch builds now report `-dev+<sha>`, and the enum is
`auto | release | continuous | development`. Workflows pass `development`
explicitly rather than falling back to `auto`, so a build's channel never
depends on how the runner happened to check the tree out.

Consequence worth remembering: **`continuous` is no longer atomic across
platforms.** Between the two uploads the release holds a Linux build from this
push and a Windows build from the previous one, and with pushes closer together
than the Windows build takes, the zip lags further — its freshness check skips
any run whose commit is no longer `origin/main`. That is the deliberate trade
for latency, and in one respect it beats the old behaviour: previously a
superseded run skipped the whole job and *both* assets went stale together.
The release notes state it, and every binary carries its own commit.

Both publishers call the new `scripts/ensure-continuous-release.sh` to move the
tag and create-or-update the release. It has to be idempotent and concurrency
safe: on a repository with no `continuous` release both callers race to create
it, so it tolerates losing that race and verifies the result instead of trusting
its own `gh release create`. The notes it writes deliberately carry no commit
line — any single one would be wrong for one of the two assets, and a
read-modify-write of a per-platform line would race between the callers.

Each publisher gets its own `concurrency` group. Sharing one would have the two
cancelling each other, since both set `cancel-in-progress`.

Asset names and the `continuous` tag are unchanged on purpose:
`bucket/syodep-continuous.json` hardcodes its download URL and CI's `jq` bump
rewrites only `version`/`hash`, so renaming either would 404 every installed
`syodep-continuous` until someone hand-edited the manifest.

Tests: `scripts/test-build-identity.cmake` drops its `preview` assertions, but
the `0.16.0+vendor` base-metadata case is kept and moved to `development` so
that path stays covered. `appimage.yml` already greps `--check` output against
`build/syodep-version.txt`, so a channel mismatch fails the build on its own.

## 2026-08-06 — A manual dispatch could publish `preview` binaries as `continuous`

`publish-continuous` gated on `github.ref == 'refs/heads/main'` alone, while
both build jobs choose their channel on the event **and** the ref. A
`workflow_dispatch` on `main` therefore built `preview`-channel binaries and
then published them as the rolling `continuous` release — a release whose
binaries report `<base>-preview+<commit>`. The comment above the condition had
claimed since it was written that "manual dispatches stop at workflow
artifacts"; the condition never implemented that.

Pre-existing, but the Scoop work sharpened it: the same job now also bumps
`bucket/syodep-continuous.json`, so a manual dispatch would have pointed
`scoop install syodep-continuous` at a mislabelled build rather than merely
mislabelling a release page. The fix adds the missing
`github.event_name == 'push'`.

Found while looking for a way to trigger a run during the Actions outage —
dispatching `release.yml` looked like the obvious workaround until the channel
expression was read alongside the job condition.

## 2026-08-06 — Preview publisher must not gate the continuous release

Fixes a regression from the entry below. Folding the preview publisher into the
release pipeline's AppImage call also folded it into that call's **result**: a
job inside a reusable workflow contributes to the caller's `uses:` job
conclusion, so a failing `publish-appimage-preview` failed `release-build-linux`
and skipped `publish-continuous` behind it.

That is not theoretical — it happened on the very commit that introduced it.
During the GitHub Actions outage of 2026-08-06 the preview job sat 15 minutes
without ever being assigned a runner and was cancelled. Both *builds* succeeded
(Linux 2m23s, Windows 13m58s), yet `a15d15f` published no `continuous` release
and no Scoop bump, because a convenience job blocked them.

`publish-appimage-preview` therefore moves out of `appimage.yml` into
`release.yml`, as a sibling of `publish-continuous` that depends only on
`release-build-linux`. Both now hang off the same build and neither can block
the other. `appimage.yml` becomes a pure builder needing only `contents: read`.

The move costs nothing in reachability: with `main` excluded from
`appimage.yml`'s push trigger, that job's `if` could only ever be satisfied
through the release call, so as a job in `appimage.yml` it was already
unreachable by any other path.

### Decision

**Preview latency is unchanged.** `needs: release-build-linux` names the Linux
build alone, so the asset still publishes without waiting on the ~14-minute
Windows job. That was the entire point of the fast preview and survives intact.

## 2026-08-06 — One AppImage build per main push, and a Windows Rust cache

`main` pushes were building the AppImage twice: once by `appimage.yml`'s own
push trigger and once through `release.yml`'s call of the same reusable
workflow. Same runner, same `ubuntu:22.04` container, same steps, same
rust-cache key — the only difference was the `SYODEP_BUILD_CHANNEL` string.
Measured on run 31054274959, the two builds took 2m22s and 2m38s in parallel.

The stated reason for the second build was latency: get an AppImage out
without waiting for the Windows job. That did not hold up. `release-build-linux`
never waited on Windows either — only `publish-continuous` does, via its
`needs:`. The job timings show the release call's build finishing at **2m30s**
while the standalone run published the preview at **3m04s**, so publishing from
the release call is if anything slightly *earlier*.

`main` is now excluded from `appimage.yml`'s push trigger, and
`publish-appimage-preview` no longer refuses to run under a release call. The
`release_call` input existed only to gate that condition and is gone.

`release-build-windows` also gained `Swatinem/rust-cache@v2`, which it never
had while CI's Windows jobs did. CMake drives cargo from the repo root with no
`--target-dir` override (`CMakeLists.txt:180`), so the workspace `target/` that
rust-cache handles is the one the build uses. For scale: CI's cached Windows Qt
job ran 11m11s against this job's uncached 13m25s on the same commit.

### Decisions

- **The `main` preview asset changes channel**, `preview` → `continuous`,
  because it is now cut from the release pipeline's build. That is what it
  always was in substance — the same binary `continuous` publishes ~11 minutes
  later. Feature-branch and manual previews stay `preview`.
- **Docs-only `main` pushes now publish a preview too**, since `release.yml`
  has no path filter. No extra build: that push already built the AppImage.

### Not verified yet

GitHub Actions was in a major outage when this landed, so the run for the
preceding commit was cancelled with no runner ever assigned. The before
numbers above are real, from completed runs; the after numbers still need a
healthy run to confirm.

## 2026-08-06 — Rebase-retry on the tagged-release manifest bump

`publish-release` now pushes its Scoop bump through the same three-attempt
rebase loop `publish-continuous` uses, so both survive `main` moving between a
job's checkout and its push — which git rejects outright. The rebase is safe in
either job: nothing else edits those manifests, and a manifest describes the
assets its own run published, not the state of the tree it lands on.

The race is far likelier for `publish-continuous`, which runs on every merge,
but a lost push costs more here: the next merge's run repairs a dropped
continuous bump, whereas a dropped release bump leaves `scoop install syodep`
on the previous version until a human notices.

`publish-release`'s checkout gained `fetch-depth: 0`, matching
`publish-continuous`. The default depth-1 clone is shallow, and rebasing needs
a merge base in local history; the boundary commit usually happens to be the
one required, but that is an accident of the common case and not something a
once-per-release path should depend on.

Both loops retry on *any* push failure, not just rejection — a permissions
failure burns three attempts before exiting 1. Accepted: it terminates and
fails loudly with git's own error in the log.

## 2026-08-06 — `[skip ci]` on the tagged-release manifest bump

`publish-release`'s Scoop bump now carries `[skip ci]` too, so the rule is
uniform: CI's own manifest commits never trigger CI. Previously only
`publish-continuous` marked its bump, where the marker is load-bearing — that
job is triggered by `main` pushes, so an unmarked bump would re-trigger itself
forever. The tagged-release bump had no such loop (its `main` push reached
`publish-continuous`, which produces a marked commit and stops), but it did
start a full Linux + Windows + installer run over a one-line JSON change once
per release.

The tradeoff accepted: after a tagged release, the rolling `continuous`
prerelease keeps pointing at the commit before the manifest bump until the next
real merge. That is a JSON-only difference in a prerelease that changes on
every merge anyway.

Not changed at the time: the tagged-release bump still pushed without the
rebase-retry loop `publish-continuous` uses. Addressed in the entry above.

## 2026-08-06 — Scoop manifest for the continuous channel

`publish-continuous` now bumps a Scoop manifest of its own, so the rolling main
build is installable and updatable the same way tagged releases already were.
`bucket/syodep-continuous.json` is a second manifest in the same bucket; it
shims `syodep-continuous` and names its shortcut "syodep (continuous)" so it
can sit alongside a stable install instead of fighting it over the shim and
Start Menu entry. `bucket/syodep.json` keeps tracking tags only — pointing the
stable manifest at rolling builds would have silently moved every existing
`scoop update syodep` onto unreleased code.

Only `version` and `hash` move: the `continuous` release assets are clobbered
in place, so the URL is constant. That makes the version the load-bearing part,
since Scoop re-downloads on a version change rather than a moved hash. It is
minted as `<base>-continuous.<YYYYMMDD>.<HHMMSS>+<sha12>`, with the date and
time split into separate components because Scoop compares numeric version
parts as 32-bit integers and a single 14-digit stamp overflows that. No
`checkver`/`autoupdate`, because a CI-minted timestamp is not recoverable from
the releases API.

### Decisions

- **`[skip ci]` on the bump commit.** The bump lands on `main`, and a `main`
  push is precisely what triggers `publish-continuous`; without the marker the
  job would publish, bump, push and re-trigger itself indefinitely. The
  tagged-release bump has never needed one because its `main` push only reached
  `publish-continuous`, which stopped there — that is exactly the property this
  change removes.
- **Push retries with a rebase.** `main` can move between the job's freshness
  check and its push. Rebasing is always clean here: nothing else edits that
  file, and a newer commit gets its own run that overwrites the manifest.

### Test strategy

The bump logic was dry-run locally against the published `continuous` zip; the
manifest is seeded with that real hash, so it is installable before the first
CI bump rather than failing verification until one lands. `extract_dir` was
confirmed against the archive: the asset is renamed at publish time but the
directory inside it is still `syodep-win64`, shared with the stable manifest.

## 2026-08-05 — Reliable focused-sidebar toggles and Escape close

Sidebar focus no longer traps the keyboard away from `<leader>a` and
`<leader>n`. The core now owns a second, isolated input state whose keymap is
derived from the active mode's effective bindings and admits only the two
sidebar toggles; custom leaders, overrides, prefix timeouts, and replay keep the
same semantics as canvas input without allowing document commands to execute
from a list. Plain Escape is unconditional and emits the new `close_sidebar`
shell request even when another sidebar sequence is pending.

The C ABI reports sidebar effects together with explicit handled/pending state,
and `CoreController` owns a separate timer so sidebar prefixes cannot disturb a
canvas prefix. `AnnotationSidebar` routes key events centrally for every
focused child and lets unmatched events fall through to its existing local
list controls. The editable Markdown box retains normal text input—important
because the default leader is Space—but Escape still closes the dock. Closing
preserves dirty drafts and reopening restores them; switching to the other
page or invoking the active page continues through the existing centralized
MainWindow toggle matrix.

### Test strategy

Core regressions cover filtering, custom leaders, active-mode overrides,
prefix timeout, count rejection, independent document/sidebar state, and an
Escape binding collision. FFI tests cover handled/pending/effect projection and
null safety. The real-window smoke test now sends actual Space+A/Space+N/Escape
events through a focused populated list and both page states, verifies
leader-shaped Markdown remains text, and closes/reopens a dirty editor to prove
the draft survives.

## 2026-08-05 — Channel-aware build versions

`--version` and `--check` now distinguish what produced the binary instead of
printing the Cargo base version for everything. A tagged release keeps the
base (`0.16.0`, including Cargo prereleases such as `0.16.0-rc.1`), rolling
main builds report `0.16.0-continuous+<commit>`, AppImage/manual previews use
`0.16.0-preview+<commit>`, and ordinary builds use
`0.16.0-dev+<commit>`. Existing prerelease identifiers are extended rather
than malformed (`0.16.0-rc.1.continuous+<commit>`). The former `build:` field
is now labeled `build type:` so its `Release` value cannot be mistaken for the
distribution channel.

`Cargo.toml` remains the only maintained base. CMake formats the identity,
writes `build/syodep-version.txt`, defines it for Qt and the Windows resource,
and passes the exact same value into the Rust compilation environment for
`syo_core_version`. The Windows installer reads the generated value rather than
independently guessing `-continuous`. Release and reusable AppImage workflows
select their channel explicitly; local/branch CMake builds infer a clean exact
version tag as `release` and everything else as `development`.

### Test strategy

A platform-neutral CMake script covers release, Cargo prerelease, continuous,
preview and development formatting. The Linux Qt CI job additionally compares
both lines of the real `--version` report with `build/syodep-version.txt`; the
packaged AppImage check makes the same shell/core assertions. An FFI regression
test covers the injected identity and Cargo-only fallback.

## 2026-08-05 — Report the database path in `--check`

The *Configuration* section of `syodep --check` now prints the resolved SQLite
database path alongside the config path. The diagnostic calls the existing
default-path helper only; it still constructs its short-lived core with
persistence disabled, so checking the path cannot create or modify the user
database.

The real-shell Wayland CI check sets a temporary `XDG_DATA_HOME` and asserts the
exact reported `syodep/syodep.sqlite3` path, covering both XDG resolution and
the CLI output without touching user state.

## 2026-08-05 — Quiet successful GL fallback and use host xkbcommon

On WSLg, Mesa can try several EGL drivers before successfully creating the
startup probe with llvmpipe. Those failed attempts wrote alarming libEGL/Zink
messages to stderr even though `--check` reported a healthy OpenGL renderer.
The Linux probe now captures its direct stderr and, after a completed frame,
removes only known failed-driver lines. Unrelated messages still pass through;
if the probe fails, every captured diagnostic is replayed before auto selects
raster or forced OpenGL exits.

The AppImage also no longer bundles Ubuntu 22.04's xkbcommon parser while
reading the user's newer host Compose table. Both `libxkbcommon` and its X11
companion are excluded and removed after Qt deployment, and an extraction check
proves the main executable resolves `libxkbcommon.so.0` outside the AppDir. This
keeps keyboard code and locale data at matching host versions without any WSL
detection, at the cost of requiring the standard `libxkbcommon.so.0` ABI on the
host.

### Test strategy

On WSLg, a successful `--renderer=opengl --check` still reports Mesa llvmpipe
and now has empty stderr. Forcing nonexistent Mesa loader and Gallium drivers
makes the probe fail, replays the EGL/Qt messages, and exits 2. The new AppImage
regression script fails against the published bundle containing xkbcommon,
passes after those libraries are removed, and is run both before packing and
after extracting the final artifact. The full Rust, lint, docs, Qt build, and
Wayland OpenGL/raster smoke gates remain unchanged.

## 2026-08-05 — Fix AppImage Wayland EGL RUNPATH

The preview AppImage contained both Qt's Wayland EGL client plugin and
`libQt6WaylandEglClientHwIntegration.so.6`, but linuxdeploy gave the manually
seeded nested plugin a RUNPATH to the AppDir root instead of `usr/lib`. On a
machine without that exact Qt library installed system-wide, including WSLg,
the plugin was discoverable but failed to load and `auto` fell back to raster
after noisy `QOpenGLWidget` errors.

Packaging now patches the plugin RUNPATH to `$ORIGIN/../../lib:$ORIGIN` after
linuxdeploy populates the AppDir. A regression script checks both the relative
RUNPATH and that `ldd` resolves the Wayland EGL client library to the AppDir's
own `usr/lib`; unlike the old check, it does not inject that directory through
`LD_LIBRARY_PATH` and therefore cannot hide a broken bundle behind the build
container's Qt installation. The extracted published artifact reproduced the
failure before the patch; the corrected extracted tree loads OpenGL on WSLg.

## 2026-08-05 — Repair reusable release permissions

The Linux release caller now grants the reusable AppImage workflow the
`contents: write` permission ceiling required by its direct-main preview
publisher. GitHub validates that nested job permission before evaluating the
`release_call` condition that skips the publisher during release builds, so the
read-only caller previously failed at workflow startup. YAML parsing and a
successful GitHub workflow start cover this CI-only fix.

## 2026-08-05 — Linux Wayland-only shell with OpenGL/raster selection

Linux now selects Qt's generic `wayland` QPA before `QApplication` and rejects
every XCB/offscreen platform override. WSL/WSLg detection, `/dev/dxg` probing,
and automatic environment rewrites to XCB/software GL are gone: a working WSLg
instance is treated exactly like any other Wayland compositor. Windows remains
a native supported target.

`--renderer=auto|opengl|raster` controls the canvas. `auto` maps a one-pixel
probe `QOpenGLWidget`, requires a valid context and one composited frame, then
uses OpenGL or falls back to a true raster `QWidget`; `opengl` makes probe
failure fatal and `raster` skips the probe. The two concrete widgets share all
painting, overlay, page-cache, DPR, key and wheel code in `CanvasState`, so the
backend cannot change document behaviour or highlight blending. `--check`
reports requested/selected renderer, probe failure, and GL strings.

Linux CI and the reusable AppImage builder now run under isolated headless
Weston instead of Xvfb/offscreen and exercise both renderers. AppImage
construction retains the separate Wayland EGL client integration, removes
every QPA plugin except `libqwayland-generic.so` and `libqwayland-egl.so`, then
verifies XCB/offscreen remain absent after extraction.

Compatibility cost is intentional: Xorg-only desktops, traditional SSH X
forwarding and X11-only VNC cannot run the Linux build. Raster fallback also
cannot compensate for a missing/broken Wayland compositor or backing store, and
uses more CPU than a working GL canvas. A context lost after the startup probe
may require restarting with `--renderer=raster`.

### Test strategy

Qt shell behavior remains compile + real-window smoke coverage. The Linux job
runs forced OpenGL and forced raster against the generated PDF, checks that
`auto` selects GL under Mesa software rendering, and asserts an XCB override is
rejected. The packaged AppImage repeats both backend smoke tests after plugin
contents and the Wayland EGL dependency closure are inspected. Rust behavior
and the C ABI are unchanged.

## 2026-08-05 — Reusable AppImage preview workflow

The production Linux AppImage builder now lives in a reusable workflow.
`release.yml` calls that workflow for `main` and tagged releases, while
**AppImage Preview** automatically builds branch pushes that change `crates/`,
`ui-qt/`, `packaging/`, or workflow files, and can manually build any selected
branch without also waiting for the Windows zip and installer. `main` runs a
second independent preview intentionally: its artifact is downloadable as soon
as the Linux job completes, rather than after the Windows build allows
`continuous` publication. Tags use only the release caller. Both paths retain
the same Ubuntu 22.04 userland, linuxdeploy bundle, plugin inspection, and
XCB/Wayland smoke tests, and upload the same `syodep-x86_64-appimage` artifact
contract.

Automatic `main` previews also update the `appimage-preview` rolling
prerelease with a raw `syodep-appimage-preview-x86_64.AppImage` asset. The
publisher confirms its commit is still the tip of `main` before replacing the
tag and asset, and is serialized so older builds cannot win a race. The
release caller passes an explicit input that suppresses this publisher: only
the direct preview workflow updates the fast Linux-only prerelease; the
existing `continuous` prerelease remains all-platform and Windows-gated.

The builder caches Rust dependencies and native dependency outputs under an
AppImage-specific key, notably avoiding repeated vendored MuPDF compilation.
Workspace crates are excluded so git-derived application identity is rebuilt
for every selected commit. Preview artifacts expire after 14 days.

Test strategy: validate both workflow files as YAML, syntax-check their embedded
Linux shell scripts, run the repository's full test/lint/docs/build/smoke gates,
then exercise the GitHub-hosted builder by manually dispatching **AppImage
Preview** and downloading the resulting artifact. The release caller preserves
the existing artifact name consumed by both publication jobs.

## 2026-08-05 — Complete AppImage Wayland EGL integration

The v0.15.1 AppImage bundled Qt's Wayland platform plugins but omitted the
separate `wayland-graphics-integration-client/libqt-plugin-wayland-egl.so`.
Consequently `QT_QPA_PLATFORM=wayland` could load the QPA backend while
reporting no client buffer integrations, and the canvas `QOpenGLWidget` could
not create a context. The release workflow now seeds that exact Qt 6.2.4
plugin into the AppDir and verifies it, its Wayland EGL client library, and its
dynamic dependency closure after extracting the finished AppImage.

WSL backend selection now prefers native Wayland when `WAYLAND_DISPLAY` is
present and retains XCB for WSL environments that expose only `DISPLAY`.
Explicit graphics environment overrides still win, and the no-`/dev/dxg`
software-OpenGL fallback is unchanged.

Test strategy: the shell smoke test asserts the three backend-decision cases
and requires a valid canvas OpenGL context on every non-offscreen platform.
The packaged AppImage originally kept an Xvfb/XCB smoke and added a
headless-Weston Wayland smoke. The later Wayland-only renderer milestone above
replaced that split with forced OpenGL and raster tests under Weston. This is
workflow and shell coverage because the integration failure is outside the Rust
core.

## 2026-08-05 — Modal keybinding-help overlay

`<C-?>` now opens a full-window semi-transparent, scrollable reference built
from the four resolved keymap tries. It shows friendly action labels plus exact
command ids, factors bindings identical across every mode into Common, groups
the remaining Normal/Focus/Visual/Highlight differences, marks the active mode,
and preserves `<leader>` spelling. Invalid or superseded config entries never
reach the snapshot.

Help is an input-isolated modal context rather than a fifth document mode.
Only `j`/`k` and arrows, half/full-page keys, `gg`/`G`, Escape, and the current
mode's effective help-toggle bindings are accepted; wheel input scrolls the
help viewport. `App::execute` also rejects every unrelated command and direct
document scrolling while help is visible, so a future palette cannot bypass
the boundary. Closing restores canvas focus without changing the underlying
mode, selection, pending highlight, or viewport.

Core tests cover effective-map factoring, custom toggle precedence, navigation
effects, modal rejection and state preservation. FFI tests cover owned snapshot
round-trips and effect bits. The offscreen Qt smoke test drives the real
Ctrl+Shift+`?` event through encoding, opens and scrolls the widget, and closes
through both supported paths. Workspace total: 680 tests.

Release metadata advances the workspace to `0.15.1` for the minor release
containing this feature. The public `v0.15.0` tag did not publish because its
packaged AppImage smoke test assumed a bare Xvfb server could focus the canvas;
`v0.15.1` preserves that tag and replaces the unpublished build. The corrected
smoke test still requires the overlay to open, populate, navigate, and close,
and asserts focus restoration on platforms that could focus the canvas before
opening help.

## 2026-08-04 — `[window] start_sidebar_open`

The closed-sidebar-on-open behaviour is now a config option rather than a
hard-coded shell policy. `start_sidebar_open` defaults to `false` (canvas
first at launch and after every document open); set it to `true` to open the
Highlights page instead. Wired like `start_fullscreen`: config → FFI →
`CoreController` → `MainWindow::applyStartSidebarPreference`.

---

## 2026-08-04 — Sidebar starts closed on open

The Highlights/Annotations dock is hidden at `MainWindow` construction and
again after every successful `openDocument`, so a newly opened file always
begins with canvas focus and no sidebar. Leader toggles, View menu, and
annotation creation (`n`) still open it on purpose.

Smoke asserts the dock is closed after construct and after open, before the
existing Highlights↔Annotations matrix.

---

## 2026-08-04 — Start fullscreen via `[window] start_fullscreen`

The main window now opens fullscreen by default. Opt out with
`start_fullscreen = false` in a new `[window]` config section.

Config lives in `WindowConfig` (`syodep-config`); the shell reads it through
`syo_app_start_fullscreen` → `CoreController::startFullscreen` →
`MainWindow::startFullscreen`, and `main.cpp` calls `showFullScreen()` or
`show()` accordingly. Window geometry stays shell-owned; the core only
carries the preference. Smoke test still uses `show()` and is unchanged.

Tests: config default/parse/unknown-field; `default_config_doc` includes
`[window]`; FFI getter reflects config and defaults to true. Docs:
`docs/config.md`, `config/default-config.toml`, `scripts/check-docs.sh`
(`WindowConfig`).

---

## 2026-08-03 — Deep-review List 3 phase 3: captions, code, reading-order fixtures

### Caption detection (`ObjectKind::Caption`)

Lines near an image/table bbox (≤24pt) with a `Fig.`/`Figure`/`Table`/`Tab.`+number
prefix, or set-apart typography, become Caption objects. Claimed after
tables/images and before headings. Block at Line/Sentence/Paragraph; auto-skipped
by Sentence/Paragraph search (footnote-like) but still a Sentence movement unit
(unlike Footnote). Tests: pure `caption_ranges_*`;
`page_content_reports_a_caption_under_{an_image,a_table}`; matrix +
`object_policy` coverage. Fixtures updated with prefixed captions near the
figure/table.

### Code-block detection (`ObjectKind::Code`)

`LineStyle.mono` mirrors `math`; Courier/Mono/… via `is_mono_font`. Runs with
mono share ≥0.6 that are not mathish become Code, claimed after equations and
before footnotes. Block, not auto-skipped. Fixture `pdf_with_code_block` /
`two_font_page_pdf(..., "/Courier")`; tests `code_ranges_*` and
`page_content_reports_a_courier_listing_as_code`.

### Reading-order characterisation

`pdf_interleaved_two_column_page` emits L1,R1,L2,R2,…;
`interleaved_two_column_page_keeps_stream_order_but_two_bands` pins current
MuPDF stream order and two x-bands without claiming a reading-order fix.

---

## 2026-08-03 — Deep-review List 3 phase 3: capabilities (items 13–17, 14)

### CJK sentence terminators

`is_sentence_terminator` accepts `。！？．`; trailers include `」』）】`. Full
CJK word segmentation remains out of scope. Tests: classifier coverage and
`cjk_sentence_terminators_split_prose`.

### Caption and code-block regions

`ObjectKind::{Caption, Code}` with claim-chain slots, config
`detect_captions` / `detect_code`, and `object_policy` entries (captions
auto-skipped like footnotes; code blocks not).

### Reading-order fixtures

`pdf_interleaved_two_column_page` characterises MuPDF stream order vs column
bands; no reading-order rewrite.

### Cross-page sentences/paragraphs

Documented as an accepted page-local limitation with a roadmap entry; no code.

---

## 2026-08-03 — Deep-review List 3 phase 2: architecture debt (items 10–12, 9)

### Shared first/last nonempty line helpers

Cross-page landings in the char/line steppers go through
`first_nonempty_line` / `last_nonempty_line`. Footnote-aware sentence helpers
stay separate.

### Object/scope policy module

`object_policy::{movement_unit, auto_skip_in_search}` is the single table for
unit-hood and Sentence/Paragraph auto-skip. `unit_object_at` and the former
`in_footnote` call sites consult it.

### ContentSession + apply_motion

`content_session::ContentSession` owns page content, furniture, extraction
failures, and derived caches. `App::apply_motion` collapses the four focus/
visual move wrappers (F-STATE-1).

### Derived per-page cache

`paragraph_segments` / `column_ranges` (after object splits) are memoized per
page on `ContentSession`; `set_page_content` drops the entry. Test:
`set_page_content_drops_memoized_derived_structure`.

---

## 2026-08-03 — Deep-review List 3 phase 1: traversal defect cluster (items 3–8)

Six defects from the traversal/segmentation review.

### Title abbreviations and initials no longer false-split sentences

`TITLE_ABBREVIATIONS` (`Dr.`, `Mr.`, …) and shape-recognised initials
(`is_dotted_initials`, new `is_single_initial` for `J.`) suppress the capital
re-enable rule via `suppresses_capital_boundary`, so `Dr. Smith`,
`U.S. Government` and `J. R. Smith` stay one sentence. Ordinary abbreviations
(`etc.`) still end a sentence before a capital. Tests: classifier coverage in
`caret.rs`; boundary cases in `app.rs` including the existing `etc.` pin.

### Drop caps are no longer headings

`heading_ranges` rejects typography candidates whose line text is a single
alphanumeric glyph. Test: `heading_ranges_rejects_a_single_glyph_drop_cap`.

### Footnotes are walkable at Sentence scope once inside

`unit_object_at` no longer treats a footnote as a block unit at Sentence
scope. Sentence auto-search uses `skip_footnote_in_search` so a caret already
inside a footnote keeps walking that footnote's sentences, while body `s`/`p`
still skip footnotes entirely. Multi-sentence fixture pins both behaviours.

### Named `s`/`p` motions update the goal row

`focus_scope_motion` / `visual_scope_motion` mirror the axis-aware goal update
from `focus_move` / `visual_move`, so a column jump after `s` aims at the new
row. Test: `named_sentence_motion_updates_the_goal_row_for_column_jumps`.

### Furniture never empties a page's last reachable line

`furniture_mask` refuses to mask a repeated match when it would be the last
remaining reachable inked line (a margin needs something to be a margin *of*).
Bottom folios under body content still mask. Tests:
`repetition_never_takes_a_pages_last_reachable_line`,
`repetition_still_masks_a_folio_under_body_content`.

### F-GEOM-1 assert and nearest paragraph fallback

`page_span_rects` `debug_assert`s on disagreeing cell indices (still shortens
in release). `paragraph_mark_containing` picks the nearest segment by line
distance instead of teleporting to the page's last paragraph. Test:
`paragraph_scope_on_an_empty_line_picks_the_nearest_segment`.

### Checks

`cargo test` for the touched suites, docs updated in `architecture.md`.

---

## 2026-08-03 — Deep-review fix batch 2: remaining List 1 bugs (items 4–11)

Eight more defects from the same review.

### Stale errors no longer stick on the status line

`last_error` is cleared at the top of `App::execute` alongside
`status_message`, so a transient failure lasts until the next command rather
than for the whole session. Test: `a_subsequent_command_clears_a_stale_error`.

### A no-op save no longer reloads the page cache

`save_document` with nothing pending reports "nothing to save: no pending
highlights" and returns `Effects::redraw()` only — no `reload`, no
`annotations_changed`. Test: `save_with_nothing_pending_does_not_reload`.

### Cancelled quit no longer replays leftover chords out of order

`dispatch` clears the input replay queue when `quit` / `confirm_quit`
interrupts a longest-prefix fallback, so leftovers cannot fire after the
dialog. New `InputState::clear_replay`. Test:
`a_quit_that_interrupts_a_replay_drops_the_leftover_chords`.

### Wheel / sidebar actions no longer cancel the pause timer

`pending_input` is stamped onto every non-key `Effects` entry point the shell
calls (`scroll_by_px`, reveal/delete/create annotation helpers) via
`stamp_pending_input`, so a prefix waiting for the pause still arms the
timer. Test: `wheel_scroll_preserves_a_pending_key_sequence`.

### Visible-page buffer grows past 64 at minimum zoom

`CoreController::visiblePages` re-calls `syo_app_visible_pages` with a heap
buffer when the count exceeds the 64-entry stack array.

### Highlight patches paint by page overlap, not containment

The Multiply path intersects each overlay rect with each visible page and
paints the intersection; rects that poke past the media box (or lose a
fraction to rounding) are no longer invisible, and a rect straddling the
page gap paints its part on each page.

### Keyboard scroll respects devicePixelRatio

Configured `scroll_step` / `horizontal_scroll_step` / `scroll_off` are
logical pixels. The shell reports `devicePixelRatio` through
`syo_app_set_device_pixel_ratio`; the core multiplies those distances by it.
Tests: `scroll_step_scales_with_device_pixel_ratio`,
`scroll_off_scales_with_device_pixel_ratio`. Documented in `docs/config.md`.

### Pending highlights preview in their captured colour

`App::highlight_overlay_groups` groups pending rects by the colour stored on
each highlight (and the in-progress placement); FFI
`syo_app_highlight_overlays` returns per-group `SyoColor` + rects; the canvas
Multiply loop paints each group in that colour. Same-colour overlaps still
blend once. Tests: `pending_highlights_group_by_their_captured_color`,
`highlight_overlays_round_trip_per_captured_color`. Architecture docs updated.

### Checks

`cargo test --workspace`, fmt, clippy `-D warnings`, `check-docs.sh`, Qt
build + offscreen smoke test.

---

## 2026-08-03 — Deep-review fix batch: FFI free layout, save rekey recovery, `<` key, backward footnote skip, multi-line span text

Five defects from a full-program review, each with a regression test.

### FFI list free had an allocator-layout mismatch (UB)

`syo_app_highlight_list` / `syo_app_text_annotation_list` leaked a `Vec` with
`as_mut_ptr` + `mem::forget`, while the free side rebuilt the allocation from
`count` alone — wrong layout whenever capacity ≠ length. Both builders now hand
out boxed slices (`Box<[T]>`, capacity == length by construction), matching how
overlays and bitmaps already did it. New multi-item round-trip test
(`multi_item_lists_round_trip_through_free`) exercises the path Miri or a
checked allocator would flag.

### A failed database rekey no longer orphans highlights and position

If the PDF rewrite succeeded but `rekey_document` failed, the blind reopen
keyed the new bytes to a fresh empty row: every highlight, annotation and the
reading position silently vanished from the session (and the "treat ids as
Embedded" recovery iterated an empty list). The rekey and the embed-marking (or
row delete, for embedded-highlight deletion) now commit in one transaction
(`Storage::rekey_and_mark_embedded` / `rekey_and_delete_highlight`), and on
failure the session re-attaches to its original row
(`App::reattach_document_row`), reloads that row's annotations, and marks the
saved ids Embedded in memory. The cross-launch integrity window remains
documented in `architecture.md`. Tests: transactional commit/rollback pairs in
`syodep-storage`, and
`failed_rekey_transaction_keeps_the_session_attached_to_its_row` in the core
(failure injected by flipping the row state behind the session's back).

### The `<` key is no longer swallowed

The shell encoded a pressed `<` as a bare `"<"`, which the chord parser rejects
as an unclosed bracket — the key vanished and could never fire a binding. The
encoder now emits the parser's escape `<<>` (and `<C-<>` etc. already worked);
`Chord`'s `Display` renders `Char('<')` bracketed so the round-trip holds.
Documented in `docs/keybindings.md`. Tests: `literal_less_than_is_written_bracketed`
plus `<<>` in the display round-trip set.

### Backward sentence motion no longer lands in a trailing footnote

`sentence_step_prev`'s cross-page branch expanded from the previous page's raw
last cell — inside the footnote block on any page that ends with one (the exact
stop the footnote auto-skip exists to prevent, only backwards), and an invalid
`cell 0` on a trailing empty line. New `last_body_cell_on_page` mirrors
`first_sentence_start_on_page`: skips trailing whitespace, empty lines, and
footnotes. Test: `sentence_prev_across_pages_skips_a_trailing_footnote`.

### Multi-line span text joins lines instead of gluing words

`span_text` emitted no separator at line or page boundaries, so a wrapped
sentence was captured as `Alpha betagamma.` in every stored anchor, sidebar
card, clipboard copy and Markdown export (all single-line tests, so unseen).
Lines now join with a single space; a line-final hyphen between two word
characters is dropped and the halves joined whole (`typeset-` / `ting` →
`typesetting`), matching `is_hyphen_interior`'s view that a line-break hyphen
belongs to the typesetting. A freestanding dash keeps itself. Tests:
`span_text_joins_lines_with_a_space`, `span_text_joins_a_line_broken_word_whole`,
`span_text_keeps_a_freestanding_line_final_dash`,
`span_text_inserts_a_space_at_page_boundaries`.

### Checks

`cargo test --workspace` (18 ffi / 40 storage / 382 core / 32 config green),
fmt, clippy `-D warnings`, `check-docs.sh`, Qt build + offscreen smoke test.

---

## 2026-08-03 — Fix Windows smoke: Markdown export without Text mode

Windows CI/release smoke soft-failed after `highlights committed`: export
round-trip compared LF core Markdown to a file written with
`QIODevice::Text`, which on Windows turns `\n` into `\r\n`. Highlights and
Annotations Markdown writers now open binary (`WriteOnly` only) so exports
keep LF. Smoke writes fail reasons into `smoke-progress.txt`; Windows CI
starts the exe with an absolute PDF path and an explicit working directory.

---

## 2026-08-03 — Fix Linux smoke: lazy Annotations panel + xvfb

CI still segfaulted on `MainWindow::show` with the Annotations widgets in the
dock tree under `QT_QPA_PLATFORM=offscreen` (Qt 6.4). The Annotations page is
now created on first show/export, and Linux CI/release smoke runs under
`xvfb-run` instead of the offscreen QPA plugin.

---

## 2026-08-03 — Fix offscreen smoke segfault with lazy Markdown preview

CI's Qt 6.4 offscreen smoke segfaulted while constructing MainWindow once the
Annotations editor eagerly built a `QTextBrowser` markdown preview beside a
failed `QOpenGLWidget` context. The Preview tab now creates `SafeMarkdownView`
on first visit. Smoke writes `smoke-progress.txt` checkpoints for triage.

---

## 2026-08-03 — Stabilize highlights and Markdown annotations (Step 8)

Reliability pass over Steps 6–7. No search, chat, tags, colors, combined export,
or new sidebar types.

### Focus and sidebar state

`MainWindow::visibleSidebarPage()` is the single read model for which page is
showing (nullopt when the dock is hidden). Menu checks and toggles use it.
Local `gg`/`dd` pending state clears on page switch, hide, canvas focus, and
document change. Opening a document focuses the canvas unless the dock was
already visible. Editor Escape: clean → annotation list; dirty → stay and
reinforce the unsaved cue.

### Dirty drafts and create ids

Dirty prompts are Save / Discard / Cancel; a failed Save aborts the transition.
`create_text_annotation` returns a stable `TextAnnotationId`; the shell selects
by id after create/save/delete (nearest-row rule after delete). Shared list
keys live in `sidebar_list_keys.h`.

### Export and persistence

Canonical Markdown is `# Highlights` / `# Annotations` with `## Page` sections.
Core strings have no trailing newline; QSaveFile writers append `\n`. When
SQLite is unavailable, annotation create/edit/delete are refused with a clear
status message (no invented session-only ids).

### Checks

Core immutability-after-save and persistence-off tests; Markdown format tests;
expanded offscreen smoke (Highlights↔Annotations matrix, create/export/delete);
`cargo test` / fmt / clippy / `check-docs.sh` / Qt build.

---

## 2026-08-03 — Independent Markdown annotations (Step 7)

Annotations are first-class objects, separate from highlights: immutable
captured source (`DocumentAnchor`) plus one editable Markdown body, never
embedded in the PDF.

### Storage

Migration v4 adds `text_annotations` / `text_annotation_rects`. The withdrawn
Step 5 `highlight_notes` schema also used version 4; open-time schema
inspection refuses a DB that has `highlight_notes` without `text_annotations`
(delete and recreate). A valid real-v4 database opens normally.

### Creation and commands

`n` (`create_annotation`) captures a frozen pending anchor from Focus (focused
unit), Visual, or Highlight (current selection) without changing mode,
selection, viewport, or the pending highlight. Normal mode reports a status
message and creates nothing. `<leader>n` toggles the Annotations sidebar page;
`<leader>a` still toggles Highlights. Empty Save is rejected; delete is
explicit only; bodies are stored exactly as entered after validation.

### Sidebar

One fixed-right dock, `AnnotationSidebar` with `HighlightsPanel` /
`AnnotationsPanel` pages. Self-toggle hides; cross-toggle substitutes; `n`
always opens Annotations in creation mode. Dirty drafts survive hide/substitute.
Shared `annotation_revision` / `annotations_changed` refreshes both lists.

### Checks

Storage schema/API tests; core create/capture/empty-body/order/persist tests;
FFI list/create; docs check; Qt build and smoke.

---

## 2026-08-03 — Keyboard-driven highlight management; comments withdrawn

Step 6. The sidebar becomes a list you can actually work from — move, jump,
delete, copy, export — and the comment feature from step 5 is gone.

### Comments removed before they shipped

The 2026-07-31 entry below describes a feature that no longer exists. One
editable Markdown body per highlight is not how a paper gets annotated, and
keeping it would have frozen a published schema in front of the threads and
agent chat that are actually wanted.

The unreleased **v4 `highlight_notes` migration was deleted** rather than
undone by a drop migration: nothing in a release ever ran it, and a second
migration would make every future database run both. The schema ends at v3.

**A development database that already ran v4 has to be deleted.** It reports
`user_version = 4`, and `Storage::open` refuses a database from a newer build
rather than downgrading it (`StorageError::SchemaTooNew`). Delete
`syodep.sqlite3` (see `docs/config.md` for the path) and syodep recreates it;
the reading position and highlights in it are lost. Test:
`a_database_from_the_withdrawn_v4_is_refused`.

Gone with it: `StoredHighlightNote`, `HighlightNote`,
`HighlightSummary::note_markdown`, `App::set_highlight_note`,
`syo_app_set_highlight_note`, the `has_note`/`note_markdown` FFI fields,
`HighlightCommentEditor`, `SafeMarkdownView`, the note roles on the list
model, the comment preview in the delegate, and the dirty-draft prompts on
selection change / open / close.

### Document order is the core's, and only the core's

`highlight_summaries()` and `all_highlights_markdown()` now return highlights
in reading order: rectangles are normalized to `(page, y0 ascending, x0)` when
they enter the app — on load from SQLite and on commit — and a highlight sorts
by its first rectangle, ties broken by row id. Page space has its origin at
the top left with `y` growing downward, which is why smaller `y0` is earlier.

Qt sorts nothing (no `QSortFilterProxyModel`, decision 20): the list, the
clipboard and the file export have to agree, and a second definition in C++
would agree only by coincidence. Tests:
`summaries_are_in_document_order_not_creation_order`,
`a_highlights_rectangles_are_normalized_into_document_order`,
`a_highlight_committed_out_of_order_still_lists_in_document_order`, and the
smoke test, which commits the second word first and checks that the list is
not in id order.

### Deleting a highlight

`App::delete_highlight` is two operations behind one name.

* **Pending** — one document-scoped row goes
  (`Storage::delete_highlight(document_id, highlight_id)`; an id from another
  document cannot delete across documents). Rectangles cascade.
* **Embedded** — the PDF copy is what the user sees, so it goes first.
  `syodep_pdf::remove_highlight_annotation` rewrites the file without the
  annotations named `syodep-highlight-{id}`, following the same
  temp-file/rename/re-key/reopen path as saving, and only then is the row
  deleted. A record whose annotation carries no matching `/NM` is refused with
  "This embedded highlight cannot be identified safely in the PDF."
  (`AppError::EmbeddedHighlightUnidentifiable`) — the alternative is guessing
  from geometry, and a wrong guess deletes an annotation somebody else made
  (decision 21).

`HighlightAnnotation` grew an optional `name`, written as `/NM` when embedding.
A highlight spanning pages produces one annotation per page and they all share
the name, so deleting takes every page of it and nothing else. Tests:
`a_named_highlight_is_written_with_its_name_on_every_page`,
`removing_a_named_highlight_takes_every_page_of_it_and_nothing_else`,
`removing_an_unknown_name_writes_nothing`,
`deleting_a_pending_highlight_removes_it_everywhere`,
`deleting_an_embedded_highlight_removes_its_pdf_annotation`,
`an_embedded_highlight_with_no_matching_name_is_refused`, plus the storage
scoping tests.

### `<leader>a`, and one place that toggles the sidebar

New command `toggle_highlights_sidebar`, bound to `<leader>a` (a bare `a` is
`highlight_enter`, and one letter must not mean two things). It carries no idea
of the sidebar's state: the core asks, `Effects::toggle_highlights_sidebar` →
`SYO_EFFECT_TOGGLE_HIGHLIGHTS_SIDEBAR` (128) →
`CoreController::toggleHighlightsSidebarRequested` →
`MainWindow::toggleHighlightsSidebar()`, which is also what `View → Highlights`
runs. `CanvasWidget` never sees the bit.

Toggling moves the keyboard, which is the point: opening focuses the list and
selects the first row if nothing is selected, closing focuses the canvas, and
`<Esc>` in the list focuses the canvas *without* closing. The dock is now
`RightDockWidgetArea` + `DockWidgetClosable` only — no floating, no moving —
and a `sanitizeDockState()` on startup undoes any restored layout that says
otherwise. There is no `Ctrl+Shift+H`.

### Sidebar keys, delete confirmation, Markdown export

`j`/`k`/arrows, `gg`/`Home`, `G`/`End`, `Enter` to jump, `dd`/`Delete`,
`y` (text), `Y` (Markdown), `Ctrl+Shift+E` (export), `Esc`. One event filter
handles them and consumes them, so nothing leaks through to the canvas and
moves the caret; `gg` and `dd` use a local two-key state machine on the core's
configured key timeout, so a stray `g` expires instead of staying armed.

Deleting asks first, and says which kind it is about to remove, because only
one of the two rewrites the PDF. Afterwards the selection lands on whatever
moved up into the deleted row (position, not id), so `dd dd` works.

Export writes every highlight to a `.md` file through `QSaveFile`, defaulting
to `<document-name>-highlights.md` beside the document, from the File menu, the
context menu or `Ctrl+Shift+E`. An empty list gets a message, not an empty file.
`syo_app_document_path` was added so the shell can name the file after the
document without knowing which one is open.

### Checks

`cargo test --workspace` (602 tests), `cargo fmt`, `cargo clippy --workspace
--all-targets -D warnings`, docs check, Qt build, offscreen smoke (now covering
document order, the export write, a delete, the dock's fixed features, and the
toggle).

---

## 2026-08-02 — Word stops across synthetic gaps; centred table columns

### Word motion: synthetic gap after `.` is not a dotted token

MuPDF often flags the gap after a full stop as synthetic. Word motion still
peeks through that gap for DOI fragments (`9p4kxc2cvd .1`), but a *letter*
after an *actual* synthetic gap is a new word (`Transformers. Unlike`,
`e.g. separating`), not an extension. The digit-only restriction applies only
when a synthetic cell is crossed — adjacent `file.txt` / `VII.0` still join.
Sentence motion was already tight here.

Tests: `a_word_stop_with_synthetic_space_does_not_glue_the_next_capital`,
`a_word_stop_with_synthetic_space_does_not_glue_a_lowercase_continuation`.
Existing DOI synthetic-space word test unchanged.

### Borderless tables: centre columns + wide multi-column grids

`alignment_table_bboxes` preferred left-edge clusters, so benchmark tables
with left-aligned headers and right-/centre-aligned numbers were missed.
Detection now falls back to centre clustering when x0 columns are unstable.
Page-wide bands with ≥3 stable columns are kept (GLUE-style grids); page-wide
two-column bands still fail closed as prose.

Tests: `alignment_table_bboxes_finds_right_aligned_numeric_columns`; existing
parameter-grid and two-column-prose tests still pass.

---

## 2026-08-02 — Traversal hardening (order + segmentation)

Audit of focus/visual word/line/paragraph motion on a real two-column journal
PDF surfaced several goal mismatches. Fixes stay document-agnostic.

### Visual enter matches focus enter

`enter_visual` / `set_head_scope` now `snap_to_scope` and
`refresh_focus_span`, so `vw`/`ve`/`vp` cannot leave a char-sized
`focus_span` while `visual_span` expands. Tests:
`visual_enter_snaps_and_refreshes_focus_span_like_focus_enter`,
`word_scope_span_never_contains_authored_whitespace`.

### Footnotes exclude math fragments

`footnote_ranges` rejects `line_is_mathish` lines in the bottom band so
`Ic,t = Ic,0`-shaped fragments are not footnotes when they miss full
equation gates. Test: `footnote_ranges_ignore_math_fragments_in_the_bottom_band`.

### List items: lone bullets + no uppercase initials

- Lone bullets pair with the nearest indented neighbour above or below
  (`pair_lone_bullet`), covering MuPDF “text then marker” emission.
- Enumerated markers accept only digits, roman, or **lowercase** single
  letters — `T. Author` is not a list. Tests:
  `a_lone_bullet_pairs_with_text_emitted_before_it`,
  `uppercase_initials_are_not_enumerated_list_markers`.

### Columns by centre when overlap merge collapses

`column_ranges` classifies seed lines by centre x; if overlap-merge still
yields one column after two-sided seeding, `center_cluster_columns` rebuilds
at the largest centre gap. Test:
`column_ranges_recovers_two_columns_when_overlap_merge_would_glue_them`.

### Furniture profile: front/back pages + page-count suffix

Sampling always includes the first/last page pairs. Normalised keys strip
trailing `Npp` / `N pages`, and mask matching accepts a longer line that
prefixes an established entry. Assertions live in
`normalise_masks_digit_runs_and_folds_case`.

### Borderless table alignment fallback

After MuPDF’s vector hunt, `alignment_table_bboxes` recovers short cell-like
grids (≥3 rows, ≥2 stable left-edge columns) and fails closed on prose /
page-wide spans. Tests:
`alignment_table_bboxes_finds_a_borderless_parameter_grid`,
`alignment_table_bboxes_ignore_ordinary_two_column_prose`.

---

## 2026-07-31 — Markdown comments on highlights

**Withdrawn on 2026-08-03 before any release — see the entry at the top. This
entry is kept as the record of what was tried and why it went.**

Step 5 after the read-only sidebar. Each stable highlight may carry one
optional Markdown comment.

### Decision: one note per highlight, not a thread

Academic annotation needs a durable note on a quote, not a chat transcript.
`highlight_notes` is a separate table (migration v4) keyed by `highlight_id`
with `ON DELETE CASCADE`. Chat will use different thread/message tables later
— notes are not message history.

Source quote (`DocumentAnchor::text`) stays immutable. The note is raw
Markdown; whitespace-only saves (`trim().is_empty()`) delete the row.

### Persistence and core

* `Storage::load_highlight_notes` / `set_highlight_note_for_document` —
  document-scoped ownership check, no-op equal body, clear on empty.
* `App::set_highlight_note` persists first, then updates memory; failures leave
  the previous comment and do not bump revision. Success emits
  `annotations_changed` without `redraw`.
* Markdown export appends the saved comment after the blockquote with one
  blank line; Pending and Embedded serialize the same way.

### Qt

* Cards show a bounded plain-text comment preview (not full Markdown).
* One `HighlightCommentEditor` under a vertical splitter: Edit (`QPlainTextEdit`)
  + Preview (`SafeMarkdownView` with `MarkdownNoHTML`, blocked resources, no
  auto-open links). No WebEngine; no per-row editors.
* Dirty drafts prompted on selection change, document open, and close (before
  Pending-highlight quit confirmation).
* "Copy as Markdown" uses Rust (saved comment only). "Copy comment Markdown"
  uses the editor draft when dirty.

### Checks

Storage/core/FFI note tests; offscreen smoke creates a highlight, saves a
comment, refreshes, and checks export. Plus workspace tests, fmt, clippy,
docs check, Qt build.

---

## 2026-07-31 — Read-only annotation sidebar

Step 4 after `CoreController`. The Qt shell now shows a dockable Highlights
panel beside the canvas.

### Architecture

```
MainWindow
├── CoreController
├── CanvasWidget
└── QDockWidget "Highlights"
    └── AnnotationSidebar
        ├── empty-state pages (no document / no highlights)
        └── QListView
            ├── HighlightListModel
            └── HighlightDelegate
```

Sources live under `ui-qt/src/sidebar/`. The sidebar talks only to
`CoreController` — no `SyoApp*`, no FFI structs, no SQLite.

### Model / view / delegate

`HighlightListModel` holds a disposable `HighlightSnapshot` and exposes roles
for id, text, colour, zero-based pages, display page label, and state label.
Full `beginResetModel` / `endResetModel` on snapshot replace — no row diffing.
Order is whatever the core returns; Qt does not sort.

`HighlightDelegate` paints a card (colour marker, page label, persistence
label, wrapped source preview) with `QStyledItemDelegate`. No child widget
per row — keeps thousands of annotations cheap and avoids focus/layout thrash
inside the list. Height comes from `sizeHint` using the same margins and line
limit as `paint` (~4 lines, elided).

### Revision-aware refresh

`AnnotationSidebar::refreshAnnotations(force)` compares
`CoreController::annotationRevision()` to the model revision and skips when
unchanged. `documentChanged` forces a refresh so a successful open cannot
briefly show the previous document's rows; failed opens do not emit
`documentChanged`, so a valid list is not cleared by a bad path. Selection is
restored by stable highlight id across resets (Pending→Embedded keeps the
same logical row selected).

### Actions

* Activate / Enter / "Go to highlight" → `revealHighlight(id)` (geometry in
  Rust; sidebar keeps list focus afterward).
* Ctrl+C → copy captured source text from the model item.
* Ctrl+Shift+C / "Copy as Markdown" → `highlightMarkdown(id)` from Rust.
* "Copy all highlights as Markdown" → `allHighlightsMarkdown()` from Rust.

Qt never formats Markdown. Empty Markdown from an error does not overwrite
the clipboard.

### Why this shape

* Disposable snapshots keep the sidebar out of the source-of-truth path.
* Stable ids prepare later comment/chat actions without redesigning the list.
* Dock is named "Highlights" under an `AnnotationSidebar` container so a
  future chat tab can share the dock without claiming the whole sidebar is
  only highlights forever.
* Embedded rows appear in the list but stay off the Pending-only canvas
  overlay (MuPDF draws them in the page bitmap).

### Checks

Offscreen smoke constructs `MainWindow`, asserts no-document sidebar state,
opens a fixture PDF, asserts empty-highlights (or list) state, exits cleanly.
Plus `cargo test --workspace`, fmt, clippy `-D warnings`,
`./scripts/check-docs.sh`, Qt build.

---

## 2026-07-31 — Shared Qt `CoreController`

Step 3 after persistent highlight records. The live window no longer talks to
`SyoApp*` from multiple widgets.

### Inventory (3.1)

Live-window FFI (moved into `CoreController`):

* lifecycle — `syo_app_new` / `syo_app_free`, default config/db paths
* input — `syo_app_key_event`, `syo_app_key_timeout`, `syo_app_key_timeout_ms`,
  `syo_app_scroll_by`, `syo_app_set_viewport`
* view/render — `syo_app_has_document`, `syo_app_visible_pages`,
  `syo_app_render_page`, `syo_bitmap_free`
* overlays — `syo_app_focus` / `selection` / `highlights`, `syo_overlay_free`
* colors/status — background/focus/visual/highlight colors, status text,
  startup warnings, open dir
* annotations — list/revision/reveal/Markdown (Step 2)
* quit — unsaved query, quit save/discard, `SYO_EFFECT_*` decoding

CLI/diagnostics (left on raw FFI):

* `syo_core_version`, default config/db paths, default config TOML
* `diagnostics.cpp` short-lived `SyoApp*` for `--check`

### Previous structure

`MainWindow` owned `SyoApp*`. `CanvasWidget` held a non-owning pointer, owned
the pending-key `QTimer`, decoded `SYO_EFFECT_*`, and emitted application
signals (`quitRequested`, `openFileRequested`, `confirmQuitRequested`,
`coreStateChanged`). `MainWindow` also called FFI directly for open/status/
quit/colors. That was fine with one interactive surface; a second (sidebar)
would have duplicated ownership and effect handling.

### New structure

```
MainWindow → CoreController (owns SyoApp*) ← CanvasWidget
                              ↑
                    (future AnnotationSidebar)
```

`CoreController` owns construction/destruction, converts every FFI allocation
into owned Qt values (`takeSyoString`, deep-copied `QImage`, `CoreOverlay`,
`HighlightSnapshot`), routes effect bits to signals, and owns the pending-key
timer. `CanvasWidget` only paints and forwards input. `MainWindow` only
composes the window and runs native dialogs. Quit confirmation uses
`quitSaving`/`quitDiscarding` with `ReturnOnly` so `closeEvent` stays
synchronous and non-reentrant. CLI/diagnostics keep their own short-lived
`SyoApp*` where they never share the live window handle.

No traversal or annotation behaviour changed in this step — only the Qt
doorway.

### Checks

`cargo test --workspace`, fmt, clippy `-D warnings`, `./scripts/check-docs.sh`,
Qt build, offscreen smoke test.

---

## 2026-07-31 — Persistent highlight records (sidebar-ready API)

Step 2 after the traversal audit. Highlights no longer disappear from SQLite
after a PDF save.

### Previous lifecycle

Commit → SQLite row → syodep overlay → save embeds into PDF → **delete the
row** → MuPDF draws the annotation.

### New lifecycle

Commit → row as `Pending` → overlay draws Pending only → save embeds only
Pending → mark those exact ids `Embedded` → keep id/text/color/geometry →
overlay stops (MuPDF draws) → summaries/reveal/Markdown still work.

Migration v3 adds `pdf_state` (`0` Pending / `1` Embedded / `2` External) and
`updated_at`. Existing rows become Pending. SQLite is the annotation-metadata
index; the PDF is the portable visual copy. Double-paint is prevented by
filtering overlays and later saves to Pending. Cross-resource failure (PDF
ok, DB mark fails) is an explicit integrity window: the session forces those
ids Embedded in memory so the same session will not re-embed; next launch may
still need reconciliation (not implemented).

Core: `HighlightId`, `DocumentAnchor`, `HighlightPdfState`, summaries,
`annotation_revision`, `Effects::annotations_changed`, `reveal_highlight`
(stored geometry only — no traversal), deterministic Markdown. C ABI:
`syo_app_highlight_list` / free, revision, reveal, Markdown (separate from
the screen-space overlay API). No Qt sidebar. Traversal behaviour unchanged.

### Tests

Storage: Pending insert, mark-by-id, rollback, v2→v3 migration, reopen.
Core: Pending→Embedded lifecycle, overlay filter, unsaved confirmation,
revision, reveal, Markdown. FFI: list/revision/reveal/Markdown/null. Full
suite including traversal metamorphic tests.

---

## 2026-07-31 — Document-traversal architecture audit

Step-1 audit before the annotation sidebar. Mapped the content pipeline,
traversal/selection call graphs, mode and cache invalidation, the canonical
ObjectKind×Scope matrix, and adjacency policies (immediate / synthetic-peek /
link-bridge). Findings and refactor candidates live in
`docs/traversal-audit.md`; `architecture.md` points there.

Low-risk hardenings landed with the audit (no sidebar, no highlight-schema
change, no reading-order rewrite):

* Extraction failure still caches empty content (non-fatal) but records the
  page and sets `last_error`, so it is distinguishable from a blank page.
* `PageContent::object_invariants_ok` (+ shared `object_ranges_ok`) for the
  sorted/disjoint/in-bounds/non-empty object contract.
* Char and line steppers, plus content-page search, skip empty lines so a
  caret cannot rest on `cell = 0` of a line with no cells.
* Metamorphic tests: empty-line skip, word count ≡ repeated steps, focus ≡
  visual word landing, visual `o` twice is identity.

### Tests

`char_motion_skips_empty_lines_between_content`,
`word_count_matches_repeated_single_steps`,
`focus_and_visual_word_motion_land_together`,
`swapping_visual_ends_twice_is_identity`;
`object_ranges_are_sorted_and_disjoint` now also asserts
`object_invariants_ok`.

---

## 2026-07-31 — Focus prefix is `f`, not `c`

The focus-mode entry chords move from `c`/`cc`/`cw`/`ce`/`cs`/`cp` to
`f`/`fc`/`fw`/`fe`/`fs`/`fp`. Bare `f` (after the pause) still enters keeping
the current scope; the scope letter is unchanged (`c` char, `w` word, `e`
line, `s` sentence, `p` paragraph). Highlight-mode fall-through that stored
via `c` now uses `f`. Visual chords (`v`/`vc`/…) are untouched.

### Tests

Config defaults and focus/highlight presses updated; existing suite covers
the chords.

---

## 2026-07-31 — Line-final colon breaks sentence and paragraph

A colon that ends its line — optionally followed only by spaces — is now a
sentence boundary and a paragraph break. `A lead-in:` then `Continued text.`
are two sentences and two paragraphs under `s`/`p`, even when the vertical gap
is too small for the ordinary paragraph heuristic. Mid-line colons
(`Note: more words.`) and colons inside links stay inert; headings already
ignore internal punctuation via the single-sentence-region guard.

Helpers live in `caret.rs` (`is_line_final_colon`, `line_ends_with_colon`);
`paragraph_segments` uses the latter, and `sentence_boundary_after` uses the
former. A list lead-in that ends in `:` is therefore its own paragraph; list
items still do not split the paragraph that follows (`a_list_is_still_one_paragraph`
updated accordingly).

### Tests

`line_final_colon_at_eol_and_with_trailing_spaces`,
`mid_line_colon_is_not_line_final`,
`colon_followed_by_an_image_is_not_line_final`,
`paragraph_segments_splits_after_a_line_final_colon`,
`paragraph_segments_keeps_a_mid_line_colon_together`,
`a_line_final_colon_ends_the_sentence`,
`a_line_final_colon_with_trailing_spaces_ends_the_sentence`,
`a_mid_line_colon_does_not_end_a_sentence`,
`a_line_final_colon_starts_a_new_paragraph`.

---

## 2026-07-31 — Zoom chords under `z`, plus `center_view`

Bare `+`/`=`/`-` no longer zoom: `z+`/`z=` zoom in, `z-` zooms out, and the
existing `zw`/`z0` stay. New `center_view` (`zc`) scrolls so the current
highlight sits at the viewport center on both axes — focus span in focus mode,
visual span in visual/highlight, remembered focus in normal mode — with true
centering (no `scroll_off` margin), Vim `zz`-style.

### Tests

`zoom_and_center_view_keybindings`, `center_view_centers_the_focus_span`;
existing zoom presses updated from bare `+` to `z+`.

---

## 2026-07-31 — Column detection ignores gutter-spanning lines

On the ST-E1 IOP paper, `h`/`l` never jumped columns on body pages:
one Keywords line, figure, or caption crossing the gutter made
`column_ranges` merge both x-bands into one, so `line_step_column`
no-oped. Detection now seeds from text lines that do not straddle the
page midpoint when both sides already look like columns, skips image
lines, merges every overlapping column (not just the first), and
coalesces nested fragments. Single-column pages still fall back to
every non-empty line, so full-width prose stays one column.

### Tests

`column_ranges_ignores_a_gutter_spanning_line`,
`column_ranges_ignores_images_when_seeding`, and
`column_ranges_merges_nested_fragments`. Existing two-column jump
tests are unchanged.

---

## 2026-07-31 — Sentence ends survive MuPDF's synthetic gaps

Same-line sentence stops on IOP two-column PDFs were being swallowed:
MuPDF often flags the gap after `.` as synthetic, and
`is_number_interior` peeked through it so `reactor. Efficacy` looked like
a dotted token (`r` + `.` + `E`). Word motion still needs that peek for
DOI fragments (`9p4kxc2cvd .1`); sentence boundaries now use a tight
neighbour check that treats any intervening space — synthetic or
authored — as ending the sentence.

### Tests

`a_sentence_ends_across_a_synthetic_space_after_the_stop` is the abstract
shape from the ST-E1 paper. Existing DOI synthetic-space word tests are
unchanged.

---

## 2026-07-31 — Committing a highlight returns to focus mode

`highlight_commit` (`a` again), and the incidental keep-and-leave paths on
save and quit, now land in focus mode on the moving end instead of visual
mode with the selection kept. They share `enter_focus`, which already stores
the pending highlight and drops the selection — the same cleanup `c` used.
Discard (`<Esc>` / `<BS>`) still restores the pre-`a` mode; explicit `v`
still goes to visual.

### Tests

`a_second_a_stores_the_highlight_and_returns_to_focus_mode`,
`saving_from_highlight_mode_keeps_the_pending_highlight`, and
`quit_commits_a_pending_highlight_before_deciding` assert `Mode::Focus`.

---

## 2026-07-31 — A numbered heading that wraps stays one heading

Section `2.3.2` in the ENDFtk article is two lines — `2.3.2. Application:
inserting the reconstructed cross section data in the` / `evaluated file` —
but sentence selection stopped after the first. The opener is flagged by
`is_numbered_heading_text`; the wrap is ordinary body-size prose with no
section number, so the post-typography numbered pass added `(i, i)` and
left the leftover as its own sentence.

Typography-flagged headings already merge adjacent same-size/weight lines;
numbered ones now do the analogous extension, but only onto lines that do
*not* fill the column (`width < 0.9 × widest`). That is what joins a short
title leftover without swallowing the full-width paragraph under a short
opener like `2.3.1. Interface overview`.

### Tests

`heading_ranges_extends_a_numbered_heading_that_wraps` is the ENDFtk shape;
`heading_ranges_does_not_extend_a_numbered_heading_into_body` pins the
short-opener / full-width-body case.

---

## 2026-07-31 — Sentence and paragraph jump columns too

Line scope's axis swap — `h`/`l` jump columns on a multi-column page, `j`/`k`
step the unit and set the goal row — now applies to sentence and paragraph
as well. Focus, visual and highlight all share `step_scope`, so one change
covers them. Horizontal motion lands via `line_step_column` then
`snap_to_scope`, so the caret ends on the sentence or paragraph that contains
the line nearest the goal row in the adjacent column. Single-column pages and
edge columns stay no-ops, same as line.

`s` and `p` keep meaning next sentence/paragraph: they now dispatch through
`Dir::Down` (like line's `e`), not `Dir::Right`, so they are not mistaken for
a column jump. Goal-row tracking in `focus_move` / `visual_move` treats
line, sentence and paragraph together.

### Tests

`sentence_horizontal_jumps_columns`, `paragraph_horizontal_jumps_columns` and
`sentence_column_jump_tracks_goal_row` on the two-column fixture. Existing
sentence prev/next tests that used `h`/`l` for unit steps now use `j`/`k`.
The fixture's cell labels gained a trailing `.` so each line is its own
sentence.

---

## 2026-07-31 — List items end where paragraphs begin

Selecting the last item of a hanging-indent list in the ENDFtk article still
ran one line into the prose that followed — twice on page 3 (the prerequisites
list ending `• Python 3.5 or higher`, and the CMake-flags list ending
`• -DENDFtk.tests=ON`), even after the same-day calibration pass below. Page 7's
list of reaction parameters was already fine.

The calibration pass only tightens the last item when some *other* item in
the page-wide marker group wraps. The prerequisites list is all single-line
items, so it had nothing to calibrate against and fell back to
`LIST_GAP_FACTOR * previous_height` at `1.5×` — about 14pt for a 9pt body —
while the inter-paragraph gap after the list is ~12pt. The indent guard
cannot fire either: the following paragraph's first line hangs to the right
of the markers, exactly like a genuine wrap. The CMake-flags list *did* have
a wrapping peer, but the prerequisites spill had already been recorded as a
"continuation" gap inside an earlier non-last item of the same marker group,
and that ~12pt poison set the calibrated cap above the real paragraph gap
that followed the second list.

Paragraph motion never had this problem: it splits on `0.75×` the page's
median line height (~7pt here), and both ~12pt gaps clear that cleanly. The
fix is to use the same factor and the same median basis for list extents.
`LIST_GAP_FACTOR` drops from `1.5` to `0.75`; `extend_item` thresholds
against the page median height rather than the previous line's own height.
A gap that opens a new paragraph now ends a list item with no extra signal.
Calibration against observed wraps stays as a *tighter* second guard for
the last item — it can only shorten further, never loosen past the
paragraph threshold.

### Tests

`the_last_item_stops_at_a_paragraph_gap_without_wrap_peers` is the ENDFtk
prerequisites shape: three single-line hanging-indent bullets, then prose
whose opener sits past the markers with a 10pt gap — under the old 12pt
threshold, over the new 6pt one. `a_later_list_is_not_poisoned_by_an_earlier_lists_gap`
pins the two-lists-on-one-page failure mode that calibration alone made
worse. Existing continuation fixtures that used a 20pt step (12pt gap, over
the new threshold) move to a 12pt step (4pt gap) so they still exercise
wrapping rather than the gap guard. The rest of the `list_items` suite and
the sentence-level list tests in `syodep-core` re-pass unchanged.

---

## 2026-07-31 — Default highlight colour: `#ffd400` at 40%

The built-in highlight default moves from the desaturated `#ffe066` at
`0.55` to pure highlighter yellow `#ffd400` at `0.4`. Over a white page
that is the same tint either way you look at it — plain alpha blend or
Multiply-then-fade both give `0.4 × #ffd400 + 0.6 × #ffffff = #ffee99` —
so the live overlay and a saved annotation stay in agreement with the
formula the colour was chosen by.

Touched: `ViewConfig::default`, `config/default-config.toml`, the FFI
fallback hex, the Qt constructor fallback alpha (`0.4 × 255 ≈ 102`), and
`docs/config.md`. No behaviour change beyond the default; existing user
configs keep whatever they set.

---

## 2026-07-31 — The furniture cap, scoped to the text that actually floods

Re-verifying "A drifted running head is still the running head" (below)
against the real document it was written for turned up a second page still
failing — not from baseline drift at all. Every baseline matched the profile
to a fraction of a point; the actual mechanism was entirely different, and
this entry is the honest correction.

The page in question carries a long run of bare `#`-normalised line numbers
from a code listing near its foot — the same shape MuPDF gives a folio. A
handful of *other* sampled pages carry similar numbered listings at broadly
similar bottom-band positions, so `build_profile` had learned several extra
`("#", Bottom, offset)` entries alongside the real folio's — false signal,
indistinguishable from the genuine one by text or shape alone. On this page,
most of those extra entries matched too, so `per_edge(Bottom)` sailed past
`MAX_FURNITURE_PER_EDGE` — and because the cap was all-or-nothing *per edge*,
tripping it discarded every Bottom match. Collateral damage was the real
finding: the page's running head and byline sit on the **Top** edge, and
should never have been touched by a Bottom-edge flood, but `too_many` was a
single page-wide bool, so one edge's flood took the whole page's evidence
down with it — the Top matches never even got to `mask[i] = true`.

The fix scopes the cap to `(edge, text)` rather than to the edge alone.
`repeated` now carries the text each line matched, not just its index, and
`too_many_for(edge, text)` counts only that specific pairing. A bare `#`
flooding the bottom is capped on its own; the running head and byline, an
entirely different text on a different edge, are judged on their own
evidence and stripped normally. The two whole-page guards — repetition
claiming every line, and the share cap — stay aggregate on purpose: a page
that is mostly repeated lines is the failure this feature exists to prevent,
regardless of how many different texts make up that majority.

One real limitation remains, deliberately not solved here: when the
*genuine* folio shares its exact normalised text ("#") with the flooding
entries, as it does on this page, there is no way to tell the two apart from
text and edge alone, so the folio itself stays capped along with the noise.
Losing a single-character page number is a far smaller failure than losing a
multi-line running head and byline, so this was judged an acceptable
trade — but a future pass could try corroborating on adjacent, *non*-folio
text sharing the exact same offset, which the flooding entries here did not.

### Tests

`mask_caps_the_repetition_rule_per_text_not_per_edge` pins the fix directly:
a running head on the Top edge plus four `#`-shaped lines flooding the
Bottom edge, past the cap. The running head survives; the flooding text
stays capped. The existing `mask_caps_the_repetition_rule_per_edge` and
`mask_caps_the_repetition_rule_by_share` re-pass unchanged — both flood a
single edge with a *single* text, which the new per-text scoping still
catches identically to the old aggregate one.

---

## 2026-07-31 — Link recognition stitches across a document's own spacing, not only MuPDF's

Re-verifying "A guessed space is not a word boundary" (below) against the
real document it was written for found the fix incomplete in a specific way:
neither the DOI in that PDF's "Program summary" box nor the abstract's own
GitHub link had *any* `SYNTHETIC`-flagged cell in them at all. Every gap —
`https://doi` |gap| `.org` |gap| `/10` |gap| `.17632` |gap| `/9p4kxc2cvd`
|gap| `.1` — was a perfectly ordinary, literal space character in the
content stream, geometrically identical in width to every real word-space on
the same line (measured directly: both came out to 2.8pt). This document's
own typesetting draws its URLs and DOIs with genuine spaces between path
segments — nobody's guess, MuPDF's or otherwise, just how the PDF was made.
Neither `synthetic` nor any geometric signal can tell such a gap apart from
an ordinary one; there is no flag to propagate here, so the earlier fix,
built entirely around MuPDF's `SYNTHETIC` bit, never had a chance to see it.

The first attempt at extending `token_span` itself to swallow such gaps
(treating a real space exactly like a synthetic one, once shape-gated)
surfaced a sharper problem first: `token_span`'s *own* synthetic-skipping
from the earlier fix had a false-positive nobody had hit yet. This document's
DOI line has exactly one `SYNTHETIC`-flagged cell in it — the space right
*before* `https`, joining the preceding prose (`files:`) onto the front of
what should have been an independent link token, corrupting the very string
`is_url_scheme` needs to see (`files:https` fails outright, since `:` is not
a scheme character). `token_span` blindly not-splitting on *every* synthetic
space, regardless of what it joined together, was always going to hit this
eventually; a real document just got there first. `token_span` now splits on
every whitespace cell again, synthetic or not — back to the simple, always-
correct contract it had before that fix.

Both problems point to the same conclusion: bridging a gap has to be a
*local, re-validated* decision made by the specific rule that benefits from
it, never a blanket "this kind of space doesn't count" applied at the
boundary-finding stage. `link_at` now does its own gap-bridging, forward and
backward, independent of `synthetic`:

- `continuing_link_fragment`/`preceding_link_fragment` admit a *following*
  or *preceding* token across a space — synthetic or real, no longer
  distinguished — only when that token's own leading character is `.` or
  `/`, the shape every continuation seen in practice shares (`.com`,
  `/njoy`, `.17632`) and an ordinary next word never does.
- Admission is not enough on its own: `link_at` re-runs `link_span` on the
  concatenation after every extension and only keeps growing while the
  match still reaches the far end of what has been gathered. A shape-gated
  candidate that turns out not to look like a link once joined (`and` +
  `/` → `and/`, which `link_span` does not recognise) simply stops the
  extension there — the false-positive backstop is doubled, shape *and*
  re-validated recognition, not shape alone.

A second, less obvious gap: the shape gate only looks *forward* from
wherever a caret already sits. `w` walking through a chain asks about the
boundary between `.org` and the space before `/10` exactly as often as it
asks about the boundary right after `https://doi` — and `token_span` queried
from inside `.org` resolves to just that one fragment, with no memory of
what came before it. `link_at` now walks backward first
(`preceding_link_fragment`, the mirror image) to find a chain's true
beginning before extending forward from there, so it resolves to the same
full span no matter which fragment of the chain the query landed in. And
because `word_run_end` steps onto the joining space cell itself as it walks
— at which point `token_span` correctly refuses to resolve *any* token,
whitespace can never be "in" one — `same_word_run`'s link check now falls
back to `link_at(right)` whenever `link_at(left)` finds nothing, so a query
starting from the space itself still lands on the right chain.

### Tests

`a_url_survives_a_real_space_inside_it` pins the base case: the real
document's own DOI shape, ordinary spaces throughout, recognised as one
link. `a_link_resolves_the_same_span_from_any_fragment_of_a_real_space_chain`
pins the backward-walk fix specifically — entered from inside `.org`, not
`https`. `a_real_space_does_not_merge_ordinary_prose_around_a_slash` is the
false-positive backstop: `and / or`, spaced on both sides the way English
prose writes it, must not weld `and` onto the bare `/` — `link_span("and/")`
does not recognise a link, so the extension never gets past its first
attempt. The existing `a_url_survives_a_synthetic_space_inside_it` and
`a_synthetic_space_does_not_merge_two_unrelated_words` re-pass unchanged: the
gap-admission rules no longer care whether a space was synthetic, but a
synthetic one still passes exactly the same shape gate a real one does, and
still fails it the same way when the neighbour isn't link-shaped.

---

## 2026-07-31 — The last list item's gap guard, calibrated

On a real article, selecting a list of prerequisite software (`\u{2022} git`,
`\u{2022} CMake 3.15 or higher`, …) ran one line past its own end: the last
bullet's region absorbed the first line of the paragraph that followed the
list. Twice on the same page, always the last item, always exactly one line.

Every item but the last stops at a hard boundary — the next marker. The last
has none, so it falls back to `list_items`'s two local geometric guards: a
gap check against `LIST_GAP_FACTOR * height`, where `height` is the
*immediately preceding line's own height*, and an indent check against the
marker's own x-position. On a hanging-indent list — markers left of the body
column, the shape "A list item always starts a sentence" already found
fragile below — ordinary prose sits to the right of the marker exactly like
a genuine wrapped continuation would, so the indent guard can never fire
there at all. That leaves the gap guard alone, and it is calibrated against
one line's height rather than against what a continuation gap in *this*
list actually looks like — loose enough (12pt here) that a modest
inter-paragraph gap (8pt) slips underneath it, while the item's own real
continuations run closer to 4pt.

The fix doesn't touch the guard for any item but the last, and doesn't touch
it at all unless there's real evidence to calibrate against. Every non-last
item that wraps is trustworthy evidence of what a genuine continuation gap
looks like in this particular list — trustworthy specifically because a
non-last item is always ultimately bounded by the next marker regardless of
how loose its gap guard is, so an imprecise guard can only ever *under*-reach
for one of those, never swallow a paragraph the way it can for the last
item. `list_items` now collects the largest such gap actually observed
elsewhere in the list and, only for the last item, adds it — scaled by
`LIST_GAP_CALIBRATION_SLACK` (1.5×, the same margin `LIST_GAP_FACTOR`
already uses, so the last item's own leading is not itself mistaken for a
break) — as a second, tighter break threshold alongside the existing one. A
list where nothing else wraps has no evidence to offer, so the last item is
left exactly as before: no behaviour change without a reason for one.

The per-item extent loop moved into its own `extend_item`, taking the new
threshold as an optional parameter, so every item can still be computed the
same way and only the last gets a second pass.

### A signal considered and rejected

The other candidate signal was geometric rather than statistical: if a
paragraph's own first line carries a typographic indent beyond its wrapped
lines' margin, the line right after a wrongly-absorbed opener should sit
*righter* than what follows it (`lines[j].x0 > lines[j+1].x0`), the mirror
image of a genuine continuation. Checked by hand against
`the_last_item_stops_where_the_list_ends` before writing any code: that
test's genuine continuation line already sits righter than the paragraph
line correctly following it — the identical shape, but correct there. That
signal cannot tell "a wrongly-absorbed opener, followed by a genuine second
line" apart from "a genuine last continuation, followed by a correctly
excluded new paragraph" without some other anchor than "whatever comes
next," and no such anchor was evident from the guards already in place.
Recorded here rather than shipped, so it isn't rediscovered the hard way.

### Tests

`the_last_item_stops_at_a_gap_the_height_guard_would_have_missed` pins the
fix: a hanging-indent, two-item list, each wrapping with a ~4pt
continuation gap, followed by prose at the continuations' own indent with an
8pt gap — under the old, uncalibrated 12pt threshold, over the calibrated
6pt one. `the_last_item_stops_where_the_list_ends` (single wrapping item, no
calibration evidence) re-passes unchanged, which is what pins "nothing to
calibrate against leaves the old behaviour alone." The rest of the existing
`list_items` suite re-passes unchanged too — none of those fixtures have a
second wrapping item, so none of them exercise the new code path at all.

---

## 2026-07-31 — A guessed space is not a word boundary

A DOI in a real article's "Program summary" box extracted as
`https://doi .org /10 .17632 /9p4kxc2cvd .1` — spaces before every path
separator that do not exist in the document's own text. Same thing in the
abstract's own GitHub link. `w` stepped through each fragment as its own
word, and `link_at` never recognised the address as a link at all, breaking
focus and selection across it.

The spaces are not syodep's doing, and not really the PDF's either: MuPDF's
structured-text extractor inserts a *guessed* space between two glyphs drawn
by separate positioning operations when the gap between them, as a fraction
of font size, looks space-shaped (`SPACE_DIST`/`SPACE_MAX_DIST` in its vendored
C). A PDF producer routinely draws a URL's path segments as separate `Tj`
runs, so this fires constantly on exactly the tokens that most need to stay
one word. MuPDF already flags every character it emits with this
information — `TextCharFlags::SYNTHETIC` — but `page_content` discarded it;
`Cell` had nowhere to keep it.

`Cell` now carries `synthetic: bool`, set from `ch.flags()` at extraction
time. Getting the caret side right took two separate changes, not one,
because the two things a synthetic space breaks are reached differently:

**Links and abbreviations** are recognised holistically — `token_span` finds
the whole whitespace-delimited run first, then `link_span`/`is_abbreviation`
look at it as a string. So the fix is at the boundary itself: `token_span`
no longer treats a synthetic-space cell as ending a token, and `link_at`/
`abbreviation_at` drop synthetic characters when building the string handed
to the recognisers (which correctly reject any whitespace) while still
returning a cell range that spans the full gap — a highlighted or selected
link has no hole in it, even though the recogniser itself never saw the gap
character.

**Numbers, dotted identifiers and hyphenated compounds** are recognised by
looking at the single character beside a separator (`is_number_interior`,
`is_hyphen_interior`) — no holistic span to lean on. `9p4kxc2cvd<synthetic
space>.1` needs its adjacency check to see past the gap: new
`prev_real_cell_on_line`/`next_real_cell_on_line` step past exactly one
synthetic-space cell. But `same_word_run` itself breaks on the very *first*
adjacent pair that fails — `('d', <space>)` — before ever reaching the real
separator two cells away, so the real-cell lookups alone were not enough;
`same_word_run` needed its own bridge across that first hop.

That bridge is deliberately narrow. `SYNTHETIC` is a general per-character
signal, not URL-specific — a document where MuPDF individually positions
every glyph (common LaTeX/Word exports) could get *ordinary* inter-word
gaps flagged synthetic too. A version of this fix that let `same_word_run`
fall through to the generic word-class rule on any synthetic neighbour would
silently weld unrelated words together on such a document. So the bridge
checks only the specific constructs already in `same_word_run`
(`is_number_interior`, `is_hyphen_interior`, `is_number_suffix_at`) on the
cell beyond the gap — never the generic `continues_word_run` fallback.
`a_synthetic_space_does_not_merge_two_unrelated_words` pins this: an
ordinary gap MuPDF happens to flag synthetic must still end the word exactly
as a real space would.

### Tests

`a_dotted_token_survives_a_synthetic_space_before_its_separator`,
`a_synthetic_space_does_not_merge_two_unrelated_words` (the false-merge
regression), `a_url_survives_a_synthetic_space_inside_it`,
`a_synthetic_space_does_not_end_a_sentence_inside_a_link` — the last one
essentially free once `link_at` was fixed, since sentence-boundary detection
already asks `is_inside_link` at every stop. New `app_with_synthetic_line`
fixture, alongside `text_line_with_synthetic`, mark specific whitespace
cells synthetic without touching the ~150 existing call sites of the
unmarked `text_line`/`text_line_at` helpers in both crates.

---

## 2026-07-31 — Footnotes, and keeping them out of the way

Footnotes had no detection at all. Footnote text at the foot of a page was
ordinary body content, spliced into MuPDF's own block-emission order — which
this codebase already uses directly as navigation order, with no separate
reading-order pass — so it could sit right in the middle of a two-column
page's flow. On a real document, one interrupted two consecutive numbered
code listings: the caret walked out of the first listing, through an
unrelated footnote's text, and into the second.

### Detection

`footnote_ranges` mirrors `heading_ranges`, inverted: a line reads as a
footnote when it sits in the page's bottom margin band *and* is set
noticeably *smaller* than the page's body size, rather than larger. Both
detectors now share `dominant_body_size` — the character-weighted-mode
computation that used to live only inside `heading_ranges` — factored out so
the two detectors' notion of "the body" cannot drift apart. Same escape
hatch as every other typography-flagged detector here: if more than half a
page's lines look like footnotes, none of them do.

Footnotes are claimed in `content_objects` *before* list items are found —
deliberately, and not merely for symmetry with the ordering of the other
detectors. A footnote's own citation text is routinely enumerator-shaped
(`12. Author, Title`), and left unclaimed it can align with, and get pulled
into, an unrelated list elsewhere on the page. `a_footnote_line_is_never_read_as_a_list_marker`
pins exactly this: a real two-item list plus a footnote whose own marker-shaped
line sits at the same indent — without the ordering, `list_items` would read
all three as one list.

### What "one stop" means here, and what "skip" needs on top of it

`ObjectKind::Footnote` takes the same four predicates as `Table` —
`is_block()` but not `is_atomic()`, `is_one_sentence()`, or a paragraph of
its own — not `Heading`'s shape, on purpose: a footnote is reachable only by
*deliberately* walking into it with word or char scope, never by stepping
through it line by line the way a heading's wrapped lines are real reading
lines.

That alone was not enough. "Must skip footnotes" turned out to mean
something `is_block()` cannot express: reading through a page's ordinary
body prose with `s`/`p` should cost a footnote *zero* stops, not the one
stop a table gets (`sentence_span_does_not_run_into_a_table` already pins
that a table costs `s` a stop of its own — the existing behavior a naive
footnote implementation would have inherited for free, and which was
exactly wrong here). None of the four `ObjectKind` predicates govern whether
Sentence/Paragraph *auto-search* stops in a region at all — they only
govern how a region behaves once a sentence run reaches it. So two new
`App` predicates, `in_footnote`, are consulted only by the auto-search loops
(`step_next_sentence_start`, `step_prev_sentence_start`,
`first_sentence_start_on_page`, `paragraph_step_next`,
`paragraph_step_prev`), never by `sentence_run_start`/`sentence_run_end`.
That split is what keeps deliberate entry unaffected: a caret walked onto a
footnote's own line with `cc`/`cw` still expands and steps through its
sentences exactly as it would anywhere else — only the automatic
forward/backward search treats the block as invisible, skipping every line
of it and continuing until it finds real body text (or the next page, via
the same cross-page fallback the unmodified search already had).

Word, char and line motion needed no code at all: a footnote stays in
`content.lines`, unlike furniture (which `skip_page_furniture` removes
outright) — it is real reading matter a reader may want, just out of the way
of ordinary reading.

### New `[view]` option

`detect_footnotes` (default `true`), following the same shape as
`detect_tables`/`detect_headings`/`detect_equations`: free to extract, a
heuristic, and independently switchable.

### Tests

`crates/syodep-pdf`: `footnote_ranges_flags_undersized_text_in_the_bottom_band`,
`footnote_ranges_ignore_ordinary_body_text_near_the_foot`,
`footnote_ranges_reject_a_page_that_is_mostly_small_type`,
`content_objects_promote_a_footnote_range`,
`a_footnote_line_is_never_read_as_a_list_marker`.
`crates/syodep-core`: `sentence_next_skips_a_footnote_block_entirely`,
`paragraph_next_skips_a_footnote_block_entirely` (both contrast directly with
the table's single-stop behavior), `word_motion_can_still_step_into_a_footnote`,
`a_sentence_started_inside_a_footnote_still_expands_normally` (pins that
deliberate entry is unaffected by the auto-search skip).

---

## 2026-07-31 — A drifted running head is still the running head

On a real ten-page article, the running head, folio and byline that were
correctly stripped as furniture on every neighbouring page came through as
ordinary body lines on one page in the middle — the caret walked straight
through "Computer Physics Communications 303 (2024) 109245" at the top of the
page before reaching any real content. Same text, same document, same
profile; one page just didn't match.

`furniture_mask` gates a candidate line through `band_of_with_folio` (is it
even in the top/bottom margin band?) and then, only for lines that pass,
compares its baseline offset against the profile's recorded offset within
`BASELINE_TOLERANCE` — 2.5 points, no aggregation, a single recorded value
from whichever sampled page first produced the entry. That page's own content
had nudged the header's baseline a few points from its siblings — plausibly
its own text pushing things down, or a MediaBox/rounding quirk — comfortably
inside the margin band but just outside the tolerance. Binary failure: no
partial credit, nothing surfaced, the header simply stopped being furniture.

The fix is not a wider band. `mask_does_not_use_the_deeper_band_for_non_folio_text`
already pins why: body text near the foot of a page that happens to share
text and offset with a profile entry must *not* become furniture, and
widening `BAND_SHARE` risks exactly that. What's actually safe to loosen is
the offset check, and only once a line has already cleared the band gate:
`furniture_mask` now tries the strict `BASELINE_TOLERANCE` first and, on
failure, retries the very same in-band `(edge, offset)` against
`RELAXED_BASELINE_TOLERANCE` (8.0pt) — never a different edge, never a
line the band gate rejected outright. Exact normalised-text equality against
an established profile entry is the corroboration that makes the wider
window safe; position is never accepted on its own.

An early version of this fix tried a band-free fallback keyed on text and an
unbanded top/bottom split, on the theory that a page's height/baseline quirk
might also push a line just *outside* the strict band, not merely off-offset
within it. It reintroduced exactly the failure
`mask_does_not_use_the_deeper_band_for_non_folio_text` guards against —
matching text at an implausible offset, outside any margin, now had a path
to being swept up as furniture. Rejected: the band gate stays exactly as
strict as it was; only the offset tolerance loosens, and only after the band
gate has already agreed.

### Tests

`mask_removes_a_running_head_whose_baseline_drifted_off_profile` pins the fix
(5pt drift, past the strict tolerance, inside the relaxed one, unambiguously
in-band). `mask_still_ignores_text_whose_offset_drifted_past_the_relaxed_tolerance`
pins that the relaxed window is still bounded (20pt drift, still in-band, but
past even the relaxed tolerance — must not match). The existing
`mask_does_not_use_the_deeper_band_for_non_folio_text` and
`mask_keeps_a_repeated_line_outside_the_bands` re-pass unchanged, which is
what confirms the band gate itself was never touched.

---

## 2026-07-31 — Scroll-off: the highlight stops short of the window edge

Walking down a page in focus mode, the highlighted line ended up flush against
the bottom border and stayed there, one line of scroll per press, with nothing
visible below it. You could never see where the sentence you were on was going.
The same at the top when moving back up. This is Vim's `scrolloff = 0`, and it
was not a decision — it was the default that fell out of "scroll the minimum
amount to make the rectangle visible".

`view.scroll_off` (default `80.0`, about three body lines at fit-width zoom) is
now the clearance the view keeps between the highlight and the top or bottom
edge. Measured in **screen pixels**, matching `scroll_step`, not in lines the
way Vim measures it: a PDF has no fixed line height, so a line-valued buffer
would change size as you read across figures, headings and body text. Pixels
also mean the strip of context is the same physical size at any zoom.

### One choke point, and a second one going the other way

Every follow-the-highlight scroll in the app — focus, visual and highlight mode
— already funnelled through `View::scroll_doc_rect_into_view`, so the margin is
one parameter threaded from `App::scroll_page_rect_into_view`. `layout.rs` stays
config-free, per its own "all math here is pure" promise: it receives pixels and
divides by zoom itself.

Two things fell out for free rather than needing code. At the document's first
and last page the buffer would push the view past the end, and `clamp_scroll`
already refuses — which is exactly the concession Vim makes at buffer ends, so
the first and last lines stay reachable flush against the edge. And a span
taller than the window minus both buffers would leave the two constraints
fighting, so the margin shrinks to `(view_h - rect_h) / 2`; past the window
height it reaches zero and a full-page figure still just pins its top edge, as
before.

The interesting half was the *inverse* direction. `reposition_focus_to_viewport`
carries the highlight along after `<C-d>`/`<C-f>`/`J`/`K`/`gg`/`G` by landing it
on the top-most visible line — which, with a buffer, is a line inside the buffer,
so the very next motion would scroll the view back and undo the jump. Both that
function and `entry_caret` (the same question when `c` enters focus mode from a
scrolled position) now ask a new `App::viewport_content_top`, which insets the
viewport top by the buffer — capped by how far the view could still scroll up,
which is what keeps the document's opening lines selectable. The two call sites
feed `topmost_visible_line`, which already meant "first line whose bottom is at
or below this y", so the inset top was the whole change.

### Tests

Five in `layout.rs` on the pure math: clearance at both edges, pixels not points
(100px at zoom 2 is 50 points), conceded at both document ends, shrinking for
tall rects, and the existing minimum-scroll test re-passed with `0.0` to pin
that opting out reproduces the old behaviour exactly. Four in `app.rs` over a
new one-page 48-line fixture — one page on purpose, since `pdf_with_line_numbers`
repeats its text and furniture detection would read identical lines on a second
page as running headers: focus and visual head both stop 80px short, `0.0` puts
the highlight flush, and `<C-d>` lands past the buffer where the unbuffered app
lands inside it. 504 tests, up from 495 (core 303 from 294).

---

## 2026-07-31 — Unit-hood became per scope: `is_atomic` from word up, `is_block` from line up

Two complaints, one root. At line scope a multi-row display equation made you
stop on every row, as if the rows of an aligned system were lines of prose. And
a highlight over an equation or a table followed the individual text lines
inside it, so a formula tinted as a ragged staircase of strips — inter-row gaps
blank, short rows ending early — instead of one rectangle.

The root was that `ObjectKind::is_atomic()` was a *single* category (tables and
images) doing two jobs at once: deciding what motion treats as one stop, and
deciding whether a highlight collapses to the object's box. One category can
only give one answer per kind, so a table was stuck indivisible at word scope
while an equation was stuck divisible at line scope, and neither could be fixed
without breaking the other.

### Two nested categories

`is_atomic()` narrowed to images alone — one stop from *word* scope up, since an
image has no words to walk. A new `is_block()` covers images, tables and
equations — one stop from *line* scope up, and drawn as one box. They nest
(atomic implies block), which the kind-matrix test now asserts outright.

Tables moved from atomic to block, which is a deliberate behaviour change: `w`
now walks the words in a table's cells instead of stepping over the whole thing.
That was the user's call, and it is the better default — a table's cells are
text you may well want a part of, whereas its *rows* are not reading lines. The
old rationale ("there is nothing useful inside them to move through word by
word") was true of images and overreaching for tables.

Which category a scope consults lives in exactly one new function,
`App::unit_object_at(page, line, scope)`: char has no units, word asks for
atomic kinds, line and coarser ask for blocks. The three call sites that
previously asked `atomic_object_at` — `scope_span`, `snap_to_scope` and the
`step_scope_atomic` skip loop — now all route through it, and the first two got
*shorter*, because the helper returning `None` for char scope subsumes the
`if scope != Scope::Char` guard both of them were carrying. `step_scope` itself,
the pure per-scope motion table, was not touched; this stays a layer over it,
per decision 14 — whose "revisit when" column predicted this exact change.

### The drawing code needs no scope, and that is not a coincidence

`page_span_rects` changed by one word: `atomic_object_at` → `block_object_at`.
It still takes no scope, and deliberately so. It already chose between one box
and per-line strips by testing whether the span covers the object from its first
character to its last — and that test is true at precisely the scopes where the
object is one unit, because at those scopes `scope_span` *returns* the object's
full extent. So the geometry question and the motion question answer each other
without either having to know about the other. Word- and char-sized spans inside
a block fail the test and still draw strips, which is what keeps a single
variable's highlight small.

Because every highlight is built from that one function, the box propagates for
free to the focus outline, the visual selection, the stored SQLite rects and the
annotation embedded into the saved PDF.

### The overhang worry, checked and dismissed

Painting a whole box risks tinting prose above or below it. Tables have a guard
for that (`table_bbox`), and it was tempting to extend it. Reading it showed why
it exists and why equations don't need it: a table's rectangle comes from
MuPDF's table detector, making it "the one object geometry not derived from the
page's own lines". An equation's box is the union of the very lines it contains,
so it reaches no further vertically than the strips already drawn today. The
change is purely horizontal — squaring off ragged rows — plus filling gaps that
are interior to the formula.

### Tests

Four existing table tests failed by design and were rewritten to the new
behaviour: word motion now walks *into* a table and `b` steps back one word
rather than out to its start; `every_coarse_scope_spans_the_whole_table` became
`every_block_scope_spans_the_whole_table` (word scope dropped, and a new test
asserts word scope spans only a word); the count test moved to line scope. The
kind matrix became a full five-kind × four-predicate table plus a nesting
assertion.

New: `pdf_with_multiline_equation` (an aligned system of three ragged rows —
the single-row `pdf_with_equation` cannot show either half of this change, since
with one row the box and the strip are the same rectangle) and a detection test
that the three rows are one object whose box is wider than its widest row. On
the core side, injecting hand-built content as the convention requires:
line motion steps over a whole equation in one press, line scope spans it end to
end, and it draws as exactly one rectangle that shrinks when you switch to word
scope. 495 tests, up from 469 — 294 core (from 281) and 142 pdf (from 129).

---

## 2026-07-30 — Dotted tokens, numbered headings, tighter equations, deeper folios

Prompted by walking the ENDFtk CPC paper (OSTI accepted manuscript) in focus
mode. Four related failures, fixed together because each one feeds the next:
version dots split sentences, missed subsection headings glued into prose,
false "equations" stole `s`/`p` stops, and folios just above the margin band
stayed in the caret path (then often became headings).

### Dotted identifiers are one word

`is_inside_number` wanted digits on both sides of `.`, so `VII.0` was three
stops and two sentence fragments. A new predicate,
`is_inside_dotted_token`, joins any full stop with alphanumeric/`_` sides and
no space — `VII.0`, `file.txt`, `a.b.c`, and `3.14` alike — and is consulted
from the same `is_number_interior` path numbers already use. `costs 3.` is
unchanged: nothing after the stop means it still ends the word and the
sentence. Grouping commas stay on the digit-only rule.

### Numbered subsection headings by shape

Typography alone misses `1.1. The ENDF format…` when it is set at body size.
`heading_ranges` now also flags lines that open with a multi-level section
number and a title (`1.1. Methods`, `2.12. Recommended…`), requiring the
trailing section-number dot so `3.14 is the value` cannot qualify. These are
added *after* the typography share cap, so a page of false bold flags cannot
erase a real subsection. Single-level `1. Introduction` stays on the
typography / list path — without an internal dot it would steal enumerated
items.

### Display equations need rich maths

Character-based detection no longer treats ASCII `+`/`=`/`<>` alone as a math
signal, and a line that is only a signed number (`−1`) is rejected. That
drops `in C++.`, `count += 1`, `-> None` and friends. Math fonts and Greek /
unicode operators still qualify; inline formulae in full-width prose remain
out via the width test, so they do not break `s`/`p`.

### Folios just above the 15% band

The ENDFtk folio sits at ~706pt on an 842pt page — a few points above the
strict bottom band — so the furniture profile was empty and every page number
was walked (and often headed). Folio-shaped lines (normalised text `#`) may
now match in a 20% bottom band while ordinary margin text keeps the 15%
band, so body lines near the foot are not pulled in.

### Tests

Pure predicates and detectors:
- `is_inside_dotted_token_*`
- `is_numbered_heading_text_*`, `heading_ranges_flags_a_numbered_*`,
  `heading_ranges_keeps_a_numbered_heading_when_typography_share_trips`,
  `content_objects_promote_a_numbered_heading_range`
- `equation_ranges_ignore_ascii_code_fragments`,
  `equation_ranges_still_find_unicode_maths_without_a_math_font`
- `a_folio_just_above_the_strict_band_is_still_in_the_folio_band`,
  `mask_removes_a_folio_just_above_the_strict_bottom_band`,
  `mask_does_not_use_the_deeper_band_for_non_folio_text`,
  `build_profile_learns_folios_from_the_deeper_band`

App motion:
- `a_dotted_version_token_is_one_word`,
  `a_dotted_version_token_does_not_end_a_sentence`,
  `a_dotted_filename_is_one_word`,
  `a_chain_of_dotted_identifiers_is_one_word`,
  `a_dotted_token_does_not_join_across_a_space`
- `a_body_size_subsection_heading_is_one_sentence_step`,
  `a_body_size_subsection_heading_is_one_paragraph_step`

---

## 2026-07-30 — Quitting now confirms unsaved highlights, and moves to `<leader>q`

Highlight mode lets a highlight live in the session and the SQLite
`highlights` table without ever being embedded into the PDF bytes — only
`save_document` (`<leader>w`) does that. Bare `q` quit immediately regardless,
discarding any such highlight with no warning, and the window's own close
button (or Alt+F4) was worse: `MainWindow` had no `closeEvent` override at
all, so it didn't even call `save_position()`. Both are fixed together, since
protecting only the keyboard path would leave the feature trivially bypassed
by clicking the window chrome.

### `q` moved behind the leader

Bare `q` is now unbound; `<leader>q` (`<Space>q` by default) is the only way
to quit, matching `<leader>o`/`<leader>w` — a deliberate two-key cost instead
of one careless keystroke that could lose unsaved work.

### `Effects::confirm_quit`, not a new `Command`

`Command::Quit` now commits any pending highlight first (the same
`store_pending_highlight` + mode-fixup pair `save_document` already runs, so
cancelling the resulting prompt can't strand the app in `Mode::Highlight`
with nothing pending), then checks `self.highlights`: nothing unsaved quits
immediately via the new `App::quit_discarding_highlights`; otherwise it
returns a new `Effects::confirm_quit` bit instead of quitting.

The two dialog answers — `App::save_and_quit` and
`App::quit_discarding_highlights` — are plain `pub fn`s, not `Command`
variants. Every existing `Command` has some default binding; these two are
only ever produced by clicking a dialog button after the core has already
decided to ask, so they don't fit the keybinding-driven registry, and adding
them there would obligate a per-mode doc-page entry for something that isn't
actually rebindable. `App::has_unsaved_highlights` is the read-only query
behind both entry points; it deliberately never commits an in-progress
highlight, unlike `Command::Quit` itself, so a caller can ask "should I warn?"
with no side effects — a real query, not a peek that changes what it's
looking at.

"Discard" does not delete anything: a highlight is persisted to SQLite as
soon as it's committed, independent of the PDF file. Discarding only skips
embedding it this time; it reappears as a pending overlay next time the
document is opened. That's what makes offering "Discard & Quit" as a
low-friction default-adjacent option safe.

### The Qt side: `closeEvent`, and a reentrancy trap

`CanvasWidget::applyEffects` falls through on `SYO_EFFECT_CONFIRM_QUIT` rather
than returning early like it does for `SYO_EFFECT_QUIT` — the confirm branch
always sets `redraw` (the mode may have just flipped `Highlight → Visual`),
and the status bar should show that before the modal dialog blocks input.

`MainWindow` gained its first `closeEvent` override and its first
`QMessageBox` (Save & Quit / Discard & Quit / Cancel), sharing one
`confirmQuit()` helper with the new `<leader>q` signal path. The window-close
path is deliberately read-only where the keyboard path is not: pressing
`<leader>q` is a command invocation and is allowed to auto-commit
in-progress work (like `save_document` already does), but the X button isn't
a command the core knows about, so the query it asks must not mutate state as
a side effect of the OS asking "are you sure" — Cancel has to leave everything
untouched. The asymmetry is intentional: cancelling a `<leader>q` prompt
leaves an in-progress highlight already promoted into `self.highlights`
(mode already flipped to `Visual`), while cancelling a window-close prompt
leaves it fully untouched, still `Mode::Highlight`.

The nonobvious bug this design avoids: "Discard & Quit" does not clear
`self.highlights` in the core, so `onConfirmQuitRequested()` calling `close()`
would re-enter `closeEvent`, see unsaved highlights again, and show the
dialog a second time — forever, on every Discard. A `m_closeConfirmed` flag
set once either path resolves short-circuits the second pass.

### Tests

Core: `quit_with_no_highlights_quits_immediately`,
`quit_with_unsaved_highlights_asks_first`,
`quit_commits_a_pending_highlight_before_deciding`,
`quit_discarding_highlights_always_quits_and_leaves_highlights_recorded`,
`save_and_quit_quits_when_the_save_succeeds`,
`save_and_quit_does_not_quit_when_the_save_fails` (281 core tests, up from
275; the old bare-`q` assertion moved out of `scroll_commands_move_and_quit_reports_effect`,
renamed `scroll_commands_move`, since `q` alone no longer does anything).
FFI: `quit_confirmation_surface` drives `syo_app_has_unsaved_highlights` /
`syo_app_quit_discard` / `syo_app_quit_save` through a committed highlight
(14 ffi tests, up from 13; 469 total, up from 462). No automated coverage for
the Qt dialog itself — no `QMessageBox` usage or headless dialog harness
exists in this repo — verified by hand under Xvfb: `<leader>q` and the window
X button each show the dialog and every button behaves correctly, both quit
immediately with nothing unsaved, and the Discard path specifically does not
loop on a second close.

---

## 2026-07-30 — A saved highlight now looks the same as its preview

The 0.11.0 release notes flagged this as a known, accepted gap: "a saved
highlight will not look pixel-identical to the alpha-blended overlay it
replaces," because a PDF reader always paints a `Highlight` annotation with
Multiply blending while the live overlay used plain alpha blending at
`highlight_opacity`. Multiply alone has no way to fade a colour, so the
saved version came out stronger than whatever the overlay had previewed.
Closing that gap turned out to need two independent fixes, one on each side.

### The PDF side: opacity was never being written at all

`mupdf` 0.7 exposes no `/CA` (constant alpha) setter — `pdf_set_annot_opacity`
exists only in the C API — and a missing `/CA` defaults to fully opaque. So
every highlight was saved at full strength regardless of `highlight_opacity`,
the same gap `/QuadPoints` had before it was written by hand into the
annotation dictionary. `HighlightAnnotation` gained an `opacity` field, and
`add_highlight_annotation` now writes it into `/CA` the same way it already
writes the quads — before `page.update()` synthesises the appearance stream,
so the generated appearance actually picks it up (`pdf_write_highlight_appearance`
reads opacity from the annotation, same as it reads the blend mode). Opacity
comes from the *current* config rather than being captured per highlight the
way colour is, since config has no hot-reload yet; the two are indistinguishable
within one session, and this avoids a schema migration for a preview-matching
fix.

### The screen side: `QOpenGLWidget` cannot be trusted with Multiply

Making the live overlay preview the same blend seemed like the obvious other
half — `painter.setCompositionMode(QPainter::CompositionMode_Multiply)` before
filling the highlight path. It compiled, and it painted the highlighted word
solid **black**. `QOpenGLWidget` paints through the GL paint engine, and the
SVG/PDF-spec blend modes beyond `SourceOver` depend on an OpenGL
blend-equation extension that is not universal; where it's missing, Qt does
not fall back to a correct result. This was caught before it shipped by
testing in this environment, but the underlying risk (driver/GPU dependent
correctness) would exist on real hardware too, not just here.

The fix composites highlights differently from focus/visual: for each
highlighted rectangle, copy the covered region of the already-rendered page
image, run `QPainter::CompositionMode_Multiply` on *that* — a `QImage`, so
the always-correct raster paint engine handles it — then draw the composited
result like an ordinary image. No GL-side blend mode is ever used. Verified
directly: a raster `QImage` probe filling `highlight_color` at `0.55` opacity
with Multiply over white paper produces exactly `(255, 238, 171)` (matching
the closed-form blend-mode-with-alpha formula by hand), and over a black
pixel produces pure black unchanged — a highlighter that tints the page but
never obscures the text under it. Comparing actual screenshots before and
after a save, the same background pixel came out `(255, 238, 171)` live and
`(255, 237, 170)` once saved and reloaded — a one-unit rounding difference,
not a colour mismatch.

### Tests

`syodep-pdf`: `a_highlights_opacity_is_written_as_constant_alpha`, reading
`/CA` back the same way `page_highlights` reads `/QuadPoints`. The six
existing `HighlightAnnotation` literals across `write_highlights`'s tests
gained an explicit `opacity` field (129 pdf tests, up from 128; 462 total,
up from 461). The Qt-side fix has no automated test — there is no headless
harness for pixel output in this repo — and was instead verified by hand:
built a minimal standalone `QOpenGLWidget` reproduction of the black-fill bug
to confirm the cause, then a `QImage`-only probe (no GL context needed) to
confirm the raster compositing math before wiring it into the real widget,
then drove the actual app under Xvfb to compare a live preview screenshot
against a saved-and-reloaded one pixel by pixel.

---

## 2026-07-30 — Margin line numbers, and headers that swap sides

Two more kinds of page furniture, prompted by a real manuscript-review PDF:
a left-margin line-number column (restarting each page), and a running
head/folio pair sharing one footer baseline that traded sides between
pages — a facing-page layout where the pair sits on the *outer* edge of
both recto and verso.

### Line numbering needs no cross-page profile

Unlike a running head, manuscript line numbers don't recur across pages —
they restart at 1 every time — so the repetition rule's whole machinery
(a document-scoped profile, multi-page sampling) doesn't apply. But
`normalise_furniture_text` already collapses any run of digits to a single
`#`, so a line that is *nothing but* a number always normalises to exactly
`"#"` — a cheap, exact test for "this line is a bare number", usable on one
page alone. `line_number_mask` finds a run of at least `MIN_LINE_NUMBERS`
such lines separated from the body text's own left edge by at least
`LINE_NUMBER_GAP`, and needs nothing else: no rows-per-page assumption, no
alignment check against a corresponding body line.

Its safety net mirrors the rotation rule's, deliberately: `body_left` is
computed only from *non*-numeric lines, so a genuinely all-numeric page (a
table of figures, with nothing to call "the body") has no left edge to
compare against and nothing gets masked — the rule can no more empty a page
than rotation can, by the same kind of construction, not a cap bolted on
after the fact.

### The repetition rule now matches segments, not whole lines

The swapping-sides header/folio turned out to already work end-to-end
without any change — MuPDF's own text extraction kept them as two separate
`ContentLine`s even sharing a baseline, so the existing per-line match (text
+ edge + baseline, x never considered) already found each one regardless of
which side it was printed on. But that was empirically true for this
fixture's PDF producer, not guaranteed by anything in the code: a producer
that emits both fields as one text run on one physical line, with a big
`Td` jump between them, would concatenate into a single string whose
left-to-right order depends on which field is on which side that page —
defeating the exact-string match even though neither field's *identity*
should depend on where it sits.

`band_lines` (profile building) and `furniture_mask`'s repetition check
(matching) now both run a line's characters through `text_segments` first,
splitting on any horizontal gap wider than `SEGMENT_GAP_POINTS` (20pt,
comfortably past any word gap at any plausible font size) before
normalising and voting. A line with one segment — the overwhelmingly common
case — behaves exactly as before; a line with two becomes two independent
candidates, so which one is on which side, or which one a left-to-right
walk reads first, can no longer change either one's identity. Masking stays
per *line*: if any segment matches, the whole physical line is furniture,
which is safe here because a margin line carrying recognised furniture
segments has nothing else worth keeping.

### Tests

`syodep-pdf` 128 tests, up from 119: `text_segments` pinned directly (word
spacing stays one segment, a wide gap splits, empty input is empty);
`line_number_mask` pinned directly (flags a real margin column, ignores a
lone stray number below the minimum, leaves an all-numeric page alone,
requires a real gutter rather than ordinary indentation); two integration
tests against new fixtures — `pdf_with_line_numbers` (numbers restart each
page, land in `content.furniture`, body text keeps reading) and
`pdf_with_alternating_margin_fields` (header and folio share one baseline
and swap sides every page; both are caught on every page regardless of
which side either is on).

---

## 2026-07-30 — `e` moves to the next line instead of a word's end

`e` used to be the odd one out: every scope-entry letter names a granularity
consistently (`cc`/`cw`/`ce`/`cs`/`cp` = char/word/**line**/sentence/paragraph),
but the standalone motion on those same letters only matched four of the
five — `w`/`s`/`p` move by word/sentence/paragraph, while `e` moved to a
*word*'s end rather than the next line, the one granularity with no
always-move-forward motion of its own. `e` is line's letter for the same
reason `ce` is (`l` is the forward motion in every mode, so line scope
couldn't use it either), so the fix is to give it the motion that matches:
`e` now moves to the start of the next line, whatever the active scope, the
same way `w`/`s`/`p` already moved by their own unit regardless of scope.

### Falls out of existing machinery, not new code

`focus_scope_motion(scope, dir, count)` was already "move by this scope's
unit, whatever the active scope is" — exactly what `w`/`b`/`s`/`p` are built
from. `e` becoming `focus_scope_motion(Scope::Line, Dir::Down, count)` (and
`visual_scope_motion` for the selection head) needed no new motion code at
all. It deleted some: `focus_word_end`/`visual_word_end` and their shared
`step_word_end_atomic`/`step_word_end` helpers existed only to give `e` its
old meaning, including the one exception to "every scope stepper lands on a
unit's start" (`Landing::End`, so a second `e` would leave a table instead of
walking back through it). That exception is gone along with the old
behavior: landing on a line's start is column 0, which is exactly what
`update_focus_goal_x` already does with any other unit's start, so a table in
`e`'s path is now skipped the same way `step_scope_atomic` already skips one
for `w`/`s`/`p` — no special case survives to describe.

`Landing::End` is not dead, though: `scope_span` still uses it to resolve the
*other* edge of an atomic object's highlight, which was always independent of
what any single motion does.

### If you rebound `e`

A `[focus_keys]`/`[visual_keys]`/`[highlight_keys]` entry naming
`focus_end_word`/`visual_end_word` will fail to load — those commands no
longer exist — and be reported as a startup warning rather than silently
kept, same as any other unknown command name. Rebind to `focus_next_line` /
`visual_next_line` for the closest available motion, or to another command
entirely if you were using `e` for something else.

### Tests

`caret_end_word_uses_current_then_next_run` became
`caret_next_line_moves_to_the_start_of_the_next_line` (asserting the new
column-0 landing from partway into a line, and that the active scope is
untouched) plus a new `caret_next_line_clamps_at_the_last_line`.
`caret_word_motions_clamp_at_document_edges` dropped its `e`-specific
assertions, since that motion is no longer word-shaped.
`word_end_lands_on_the_table_end_then_leaves` became
`next_line_treats_the_table_as_a_single_step`, asserting a `Landing::Start`
arrival (the table's first line) instead of `Landing::End` (its last) before
the following `e` clears it — the existing highlight/visual reshape-parity
test already exercises `e` as one of its nine keys, so it needed no change:
both modes route it to the same renamed command.

275 core tests (was 274 — one net test added), 30 config, unchanged elsewhere.

---

## 2026-07-30 — `open_file` moves from `<C-o>` to `<leader>o`

`<C-o>` was the open-file binding since it moved off a bare `o` in the
2026-07-28 entry below, back when there was no leader key to move it to. Now
that one exists (added for `save_document`'s `<leader>w`), grouping the
application-level commands under it is the more consistent home: `<leader>o`
opens, `<leader>w` saves.

The empty-state status line ("no document - press ... to open a PDF") moved
with it, for the same reason it moved off `o` in 2026-07-28: it must never
advertise a key that no longer does the thing it says.

If you rebound `<C-o>` yourself, that binding still works — user `[keys]`
entries extend the defaults rather than replacing them — you only lose the
*default* binding, which is now `<leader>o`. `<C-o>` itself is unbound again,
and free for the `<C-o>`/`<C-i>` jump-history keys on the roadmap.

---

## 2026-07-30 — Highlight mode, and saving the PDF with highlights in it

Phase 2 item 3. `a` from focus or visual mode turns what is focused or selected
into a highlight; it stays adjustable with every visual-mode motion; `a` again
keeps it, `<Esc>`/`<BS>` throws it away and restores exactly what you had, and
`v`/`c` with any specifier keep it on the way into the mode they name. A new
`<leader>w` (leader `<Space>`) overwrites the PDF with every stored highlight
embedded as a real PDF `Highlight` annotation.

### The whole feature is three commands and no motion code

`visual_move`, `visual_scope_motion`, `visual_word_end`, `swap_visual_ends` and
`set_head_scope` guard on `self.visual.is_some()`, not on `self.mode ==
Mode::Visual`. So `[highlight_keys]` binds `hjkl`, `w`/`e`/`b`/`s`/`p`, `o` and
`o{scope}` to **the `visual_*` commands themselves**. A pending highlight stores
no extent of its own either — it *is* `visual_span` — so `refresh_visual_span`
was not touched. Entering from focus mode synthesises the second end, which is
what lets `w` grow a highlight that started on one focused word.

Only `highlight_enter`, `highlight_commit` and `highlight_discard` are new. The
invariant is pinned by a test that runs nine key sequences through both modes and
asserts the spans match, so a key cannot come to mean different things in the two.

`PendingHighlight` records the mode, position, scope and anchor to put back, and
discarding is those four assignments — there is no partially-applied state,
because entering only ever *adds* an anchor and never moves the position. The
exits that keep a highlight all call one `store_pending_highlight`, so "which
exits keep it" is a fact about the keybindings rather than a condition in four
places.

`enter_visual` needed one early branch: its ordinary path collapses the selection
onto the head, which is right for `v` inside visual mode and would throw away the
extent the user just shaped when coming out of highlight mode. That is the one
regression this design could plausibly have, so it has its own test.

### A highlight is rectangles, not a span

Stored as page-space rectangles plus the covered text (decision 15). That serves
all three consumers — the overlay, the PDF's `/QuadPoints`, and later export —
and means a stored highlight draws on reload with no page content extracted at
all.

Decision 13 anticipated this: the overlay was computed per *visible* page and
said to revisit "when selections need to be exported/persisted whole". Doing it
removed duplication rather than adding any: the per-line geometry moved into a
pure `caret::page_span_rects`, and `span_screen_rects` (visible pages) and the
new `span_page_rects` (every covered page, extracting as needed) are now two
callers of it. The `#[cfg(test)]` `span_text` helper became a real method for the
same reason, so 79 existing tests now also pin what a highlight records as its
text.

### Writing the PDF: two things MuPDF does not let you do the obvious way

`PdfAnnotation::set_rect` **raises** for a highlight. MuPDF lists the subtypes
whose `/Rect` is settable (`rect_subtypes` in `pdf-annot.c`) and computes a
quad-point annotation's rect from its `/QuadPoints` instead. And the `mupdf` 0.7
crate exposes no quad-point setter at all, though the C API has one.

So `write_highlights` creates the annotation, then writes the quads into its
dictionary through the page's `/Annots` at the index recorded *before* creating it
(not "it's the last one"), transformed by the inverse page CTM — which is what
`pdf_set_annot_quad_points` does internally, and what keeps rotated pages right
where a bare `height - y` would not. `page.update()` comes after that edit, since
it is what synthesises the appearance stream. All still safe Rust; no new
dependency.

It is a free function opening its own handle rather than a `Document` method:
`PdfDocument::try_from` consumes the document by value, the live one is busy
rendering, and a failed write then cannot leave it half-annotated.

**Embedded highlights cannot disturb the content layer**, which was the risk
worth checking: text extraction goes through `fz_run_page_contents`, so
annotations are skipped, and the `COLLECT_VECTORS` table hunt cannot mistake a
highlight's appearance rectangles for a ruled table. A test asserts the extracted
lines and objects are byte-identical across a save, so that is pinned rather than
believed.

### Saving overwrites in place, and re-keys the document

Write beside the original, then rename over it — atomic on one filesystem, so an
interrupted save can never leave a half-written PDF where the document was. The
session is dropped *before* the rename, because on Windows replacing a file MuPDF
still holds open fails with a sharing violation, and CI runs the Windows suite.

Rewriting changes the file's hash, and the hash *is* the document's identity, so
`rekey_document` moves the row to the new fingerprint as part of the save
(decision 16). Without it every save would silently orphan the reading position.
Then the highlight rows are dropped and the overlay stops drawing them: they are
annotations MuPDF renders now, and drawing both would paint them twice. They do
look slightly different afterwards — a PDF highlight blends Multiply rather than
using `highlight_opacity`.

`Effects::reload` / `SYO_EFFECT_RELOAD` is new because the shell's page-image
cache is only invalidated on a width mismatch, so after a same-zoom reload it
would keep serving pre-save bitmaps. The same latent bug was in
`MainWindow::openDocument`, which never cleared the cache either; both now call
`CanvasWidget::clearPageCache()`.

### The leader is expansion, not a mechanism

`[input] leader` (default `<Space>`) plus `parse_sequence_with_leader`, which
splices the leader's chords in place of `<leader>`. By the time the keymap sees
it, `<leader>w` is an ordinary sequence and disambiguates by the existing prefix
rules — the input state machine is unchanged. The leader lives on the `Keymap`, so
the mode keymaps (clones plus an overlay) cannot end up with a different one from
the table they extend. `parse_sequence` rejects a bare `<leader>`, which is also
what stops `leader = "<leader>"` recursing, since the leader itself is parsed with
it. An unparseable leader warns and falls back rather than costing the user every
`<leader>` binding.

### Tests

Storage: 6 new (migration v2 from scratch and from a populated v1, multi-page
round trip, per-document scoping, both cascades, re-key carrying position and
highlights). PDF: 6 new (source untouched, quad geometry round-trips through
bottom-left user space, empty case, content layer unchanged, the highlight
actually paints yellow on the page, out-of-range page rejected). Core: 18 new
covering entry from both modes, the reshape-parity invariant, commit, both undo
keys including after reshaping, the focus-entry anchor being uninvented, `v`/`c`
exits, the collapse regression, `a` inert in normal mode, no document, spanning
pages, reload from the database, and the save path end to end — file rewritten,
annotation present, position and selection preserved, rows dropped, temp file
gone. Plus config, keys and FFI tests. 451 total, up from 418.

The failing-save test is `#[cfg(unix)]` and forces the failure by removing write
permission from the directory. Worth recording why: the obvious trick — putting a
directory where the temporary file goes — does not work, because MuPDF *removes*
whatever is at the path it is told to save to.

---

## 2026-07-29 — Equation fixture text is platform-dependent

Windows CI failed `page_content_detects_a_display_equation`: MuPDF there
extracts the Symbol fixture as Latin `a + b = g`, while Linux gets Greek
`α + β = γ`. Detection still found the equation (font name), but the test
asserted on Greek glyphs. The integration check now accepts either decoding;
the pure `equation_ranges` tests keep using Greek strings directly.

---

## 2026-07-29 — Display equations are navigation units

A formula used to be a short line of odd characters: `s` walked into it, it was
glued to the sentence before it (equations rarely end in a full stop), and a stop
inside one split it. Now a display equation is **one step at sentence and
paragraph scope** while `w` and `h`/`l` still walk through it — the behaviour a
heading already had, asked for by name.

### Almost no new mechanism

`docs/architecture.md` claimed that adding a kind means answering the region
predicates rather than threading anything through the motion code. This feature
tested that claim, and it held: `ObjectKind::Equation`, added to `is_atomic()`'s
exception list, and a third predicate `is_one_sentence()` (`Heading | Equation`)
replacing the hard-coded `ObjectKind::Heading` test in `App::in_heading`, renamed
`in_single_sentence_region`. **No motion code changed.** The rest of the work is
the detector, its config toggle, and tests.

The third predicate earns its place: without it a two-line equation whose first
line ends in `.` is two sentences. Pinned by removing `Equation` from it and
watching `a_stop_inside_an_equation_does_not_split_it` and
`sentence_motion_treats_an_equation_as_one_step` fail — which is how the first
version of that fixture was found to be worthless, since its only unprotected
stop sat at the region's end where a boundary changes nothing.

### Two math signals, and the guard that matters

`LineStyle` gained `math`, the share of a line's glyphs set in a math font, read
from `ch.font()` in the pass that already extracts the text (with the last font
name and verdict cached, since glyphs come in font runs). `is_math_font` is a
case-insensitive substring test — `cmmi`, `cmsy`, `cmex`, `msam`, `msbm`, `stix`,
`xits`, `symbol`, `math` — which covers TeX's families, the Unicode math fonts
and the `ABCDEF+` subset prefixes in one rule. MuPDF's source says this is
reliable: every load path passes the PDF's own font name to
`fz_new_font_from_memory`.

The characters are the second signal, for PDFs whose fonts say nothing:
operators, relations and Greek. Either signal is enough — and both were verified
*independently* against the fixture, by disabling one at a time and watching
detection still succeed.

A line is an equation when **all four** hold: it does not fill the column, it
reads as mathematics by fonts or characters, it carries an operator or relation,
and it carries at most two ordinary words (a word being three or more letters, so
adjacent variables do not count). Each condition kills a specific
counter-example: a prose sentence with inline maths (full width), a centred
caption (no operator), a citation line (words), the last short line of a
paragraph (words, no operator).

**Inline maths is deliberately out of scope**, decided with the user: for a
formula inside a sentence to be one stop it would have to be a region, and a
region splits the sentence around it — the cure would be worse than the disease.
The set-apart test is what enforces that, and it is the guard most worth keeping
honest.

Runaway guards mirror the heading ones: at most 12 lines to an equation, and if
more than 60% of a page's lines read as maths, none of them do. The share guard
is loose because an appendix page legitimately is mostly display math; when it
fires, the page just navigates line by line.

### The fixture trick

Testing a font-based heuristic without embedding a font: the new
`pdf_with_equation` sets its formula in base-14 **`Symbol`**, so MuPDF reports
the font name `Symbol`. On some MuPDF builds the encoding also turns
`a + b = g` into `α + β = γ`; on others (notably Windows CI) the Latin bytes
survive. The font signal alone is enough for the integration test. The
6-object assembly is now shared with `pdf_with_heading` as `two_font_page_pdf`.

13 detection tests (10 pure on `equation_ranges`, 3 on real MuPDF) plus 6
behavioural tests in the core against a hand-built `PageContent`, so a future
change to the heuristic can only fail the detection tests.

---

## 2026-07-29 — A table stops at its last row, not at MuPDF's rule

Reported from real use: at sentence scope the highlight on a table reached over
the prose line below it, and `s` **skipped** that line instead of landing on it.
The skip is the diagnostic — it means the line was inside the table's line
range, part of the atomic unit, rather than merely painted over. Everything
downstream reads that range, so the caret, the sentence run and the overlay were
all wrong together.

Cause: `content_objects` decided membership with one rule — a line belongs to
the table if its **centre** falls inside MuPDF's box. That box is the *ruled
region*, which reaches past the last row of text; when it reaches past the next
line's centre, prose joins the table. Measured on the fixture at a 4pt caption
gap: box `y 141.65..282.35`, caption line `y 275.25..288.99`, centre `282.1`,
inside the box by a quarter of a point. That is all it takes.

Two things had to change, and only the first fixes the skip.

**The range is trimmed at its edges.** Two tests, applied only at the first and
last member — a box cannot clip an interior line, and an interior gap is a real
part of the grid:

- a line lying inside the box by less than 70% of its own height
  (`TABLE_MEMBER_OVERLAP`) is one the box merely cuts through;
- with three or more members, an edge line whose adjoining gap exceeds 1.8×
  (`TABLE_GAP_FACTOR`) the **median** gap of the members is set apart from the
  grid — a caption, or the prose after it.

The normalisation is the interesting part. The gap is compared against the
table's own rhythm, not against line height: `pdf_with_table` sets 10pt text on
28pt rows, so a line-height comparison would have eaten its first and last row.
The shape of the test, and its constant, mirror the list walker's
`LIST_GAP_FACTOR` — "a wide gap means the block below merely follows the list".

Trimming can only shrink a table, and one shrunk below two lines falls to the
existing discard guard, degrading to line-by-line navigation — always the safe
direction here.

**The stored box is derived from the lines.** Tables were the only object kind
whose `bbox` was foreign geometry (`bbox: *table`); headings use the union of
their line boxes and images the line box. The box is now extended to cover the
rows' text and then held back from the nearest non-empty line above and below,
so it cannot reach a line the caret can visit. The neighbour deliberately wins
over the table's own text extent: line boxes include ascenders and descenders
and overlap each other by up to a point, and stopping a point short of a
descender is invisible where tinting the line below was the whole complaint.
Without this second half the skip would have been fixed and the tint would have
remained — measured, at that same 4pt gap, as 3pt of overhang into the caption's
box.

### Tests

Five pure cases on `content_objects` (each edge trimmed, the gap test with a box
that covers the caption *entirely* so the overlap test says nothing, a
generously spaced table keeping every row, and a table trimmed below two lines
being discarded), plus one against real MuPDF on a new fixture:
`pdf_with_table_gap(cols, rows, caption_gap)`, with `pdf_with_table` now a
wrapper at the old 40pt. The gap for the fixture test was chosen by measurement,
not guessed: at 8pt MuPDF's box overhangs the caption's *box* but not its centre
(the tint bug alone), and at 4pt it takes the caption into the range (the skip
bug too). The test was watched failing on both assertions with the fix bypassed.

No core change. `same_region` / `next_cell_in_region` were already stopping
sentence runs at region edges correctly — the edge was in the wrong place.

---

## 2026-07-29 — A link is one word

`https://example.com/a?x=1` was a dozen stops for `w`, and worse, every dot in
it ended a sentence. It is now a single word, and a stop inside it is inert.

Token-level, like the abbreviation rule, and it reuses that rule's shape
exactly: `link_span` recognises the address inside a whitespace-delimited token
and returns its span; the span is a hard edge in both directions for
`same_word_run`, and `sentence_boundary_after` treats a terminator inside it as
part of the address. `abbreviation_at` and the new `link_at` now share
`App::token_span`, which is the only thing that knows where a token begins and
ends.

The recognised forms are a scheme (`://` with a plausible scheme in front, or
`mailto:`/`doi:`/`tel:`/`urn:`/`arxiv:`), a `www.` host, a dotted host followed
by a path, and a plain email address.

**What is deliberately not recognised is the interesting part.** A bare
`example.com` with no path and no `www.` is rejected, because extraction that
drops a space leaves `sentence.Next` looking exactly like it — and accepting
that would both glue two words together and swallow a real sentence boundary.
Requiring a path or a `www.` prefix costs almost nothing (a bare host in a paper
is rare) and removes the whole class of false positives. The host test also
wants an alphabetic top-level domain, which keeps `Fig.2/3`, `10.1000/182` and
`1,234.56` out, and the missing dot keeps `and/or`, `km/h` and `src/lib.rs` out.

Trailing punctuation is trimmed before recognition, with one refinement: a
closing bracket is trimmed only when the link did not open one itself, so
`…/Glob_(pattern)` keeps its `)` while `(https://example.com),` gives up both.
Sentence-ending punctuation after a link is therefore outside its span and still
ends the sentence, which is what `a_stop_after_a_url_is_its_own_word_and_ends_
the_sentence` pins.

The cell mapping goes through the characters actually present in the token, so an
image sitting inside one cannot shift the span — the same trap `abbreviation_at`
avoids by trimming on cells rather than on the string.

Nine motion tests, two unit tests on `link_span` (one of them entirely about what
must *not* match).

---

## 2026-07-29 — Scientific notation and percentages are one word

Two more shapes a figure takes in a paper, both extensions of the existing
number rule rather than new machinery.

**The exponent sign.** `1.5e-10` was already one word — but only by accident of
the hyphen rule landing first, since `e` and `-10` are word characters either
side of a hyphen. `2.3E+5` was three stops, because `+` had nothing joining it.
`is_number_interior` now also accepts an exponent sign, tested by
`is_inside_scientific_exponent(before2, before, sign, after)`: an `e`/`E` behind
the sign, a **digit** behind that, and a digit after. The digit is the whole
point — without it `cache+1` would collapse into one word. `+`, `-` and the
Unicode minus all count, since a typesetter may have set either.

**The proportion sign.** `45.5%` was a number and a symbol. A `%` (or `‰`, `‱`)
with a digit immediately before it now joins backwards. Only the right-hand cell
of the pair is ever asked, which is what keeps the join one-directional: the sign
attaches to its figure and never reaches forward into what follows. `the % sign`
and `45.5 %` are untouched, both for want of a digit in front.

Units are deliberately excluded. `°` looks like the same shape, but it is
followed by a letter (`37°C`), and joining the sign but not the letter gives
`37°` + `C`, which is worse than leaving it alone — while joining the letter too
opens the whole question of unit tokens. The set is proportion signs only, and
`is_number_suffix` says so.

Nothing changed on the sentence side: neither construct contains a terminator,
so `It grew 45.5%. Then more.` splits on the stop after the sign, as it always
did. Seven motion tests plus two on the predicates.

---

## 2026-07-29 — An abbreviation is recognised inside the punctuation around it

`(e.g.,` — the commonest form the construct takes in a paper — was not being
recognised at all. `abbreviation_at` matched the whole whitespace-delimited
token, and `(e.g.,` is not in any list and is not a run of dotted initials, so
every rule keyed on it was off: the stops split the sentence, and `w` stopped
four times inside it.

The token is now trimmed to the construct before it is recognised: leading
characters that are not word characters go, and trailing ones go too *except* a
full stop, which may be the abbreviation's own. `(e.g.,`, `[etc.]`, `"i.e."` and
`etc.)` all reduce to the abbreviation itself, and the span returned is that
core rather than the token.

Trimming alone gave the sentence rule what it needed but left `w` wrong in the
other direction: the closing stop of `e.g.` and the comma after it are both
punctuation, so `continues_word_run` merged them into one run — `e.g.,` as a
single stop. The span is now a hard edge in **both** directions: when either
side of an adjacent pair is inside it, they belong to the same run only if both
are. `(e.g.,` is three stops, which is what it looks like.

Two consequences worth recording:

- The span no longer contains every cell of the token, so `is_abbreviation_stop`
  must check that the stop it was handed is actually inside it. Without that
  check the `)` in `…magic (etc.)` would inherit the construct's inertness and
  swallow a real sentence boundary — pinned by
  `a_bracket_after_an_abbreviation_can_still_close_a_sentence`.
- The capitalisation test now has a veto: a comma, semicolon or colon reached
  before the capital means the text runs on. This is what a citation needs —
  `(e.g., Smith 2020)` would otherwise split at the abbreviation, since `Smith`
  is capitalised. Continuing punctuation is a stronger signal than a capital,
  and only where an abbreviation is already suspected.

Six new tests, four of them written first against the reported behaviour:
`a_bracketed_abbreviation_is_one_word_between_its_brackets`,
`a_bracketed_abbreviation_does_not_break_a_sentence`,
`an_abbreviation_before_a_comma_keeps_the_sentence_even_before_a_capital`,
`a_quoted_abbreviation_is_still_one_word`, plus the bracket-closing case and a
unit test on `opens_a_sentence`.

---

## 2026-07-29 — A hyphenated compound is one word

`well-known` was three stops for `w`: the two halves and the hyphen between
them. It is now one, in the same shape as the number and abbreviation rules —
a pure predicate, `is_inside_hyphenated_word(before, hyphen, after)`, consulted
from `same_word_run` through `App::is_hyphen_interior`, exactly as
`is_inside_number` is.

The test is word characters (alphanumeric or `_`) on **both** sides, which is
what separates a hyphen from a dash without needing to know anything about the
words themselves. It composes, so `state-of-the-art` holds together throughout,
and it covers `COVID-19` and `3-D` for free. Everything that is not doing that
job keeps the stop it always had: `one - two`, a trailing `well- known`, and
`one--two` (neither hyphen has a word character on both sides).

Which characters count as a hyphen is the whole judgement call. `-` plus the
typographic hyphens `‐` (U+2010) and `‑` (U+2011), plus the soft hyphen
(U+00AD), which a PDF can carry where a word was set to break. The en and em
dashes are deliberately excluded: `one—two` is a clause boundary, not a compound,
and joining it would swallow two words into one stop. Pinned by
`an_em_dash_between_words_does_not_join_them`.

Sentence and paragraph logic is untouched — a hyphen was never a terminator, so
there was nothing to make inert, unlike the stops inside `3.14` and `e.g.`.

The one case left alone is a word hyphenated across a line break. Word runs stop
at a line boundary by construction (`same_line`), and the same-line guard here
mirrors the number rule's; rejoining such a word would mean reaching into the
next line and deciding whether the hyphen was the author's or the typesetter's.

`word_right_and_left_step_between_words` used `beta-gamma` as its example of a
punctuation stop between two word runs, which this rule dissolves; it now uses
`beta:gamma` and asserts the same cell indices. Nine new tests cover the motions
(`a_hyphenated_compound_is_one_word` and neighbours) plus two unit tests on the
predicate itself.

---

## 2026-07-29 — Darker, less transparent default overlays

The shipped highlights were too faint to find on a page: light blue `#add8e6`
and light grey `#d3d3d3`, both at `0.4` opacity, blend over a white page to
`#dceff5` and `#ededed` — a few percent off white. The defaults are now the
mid-tone `#5b9bd5` (focus) and `#8a8a8a` (selection) at `0.55` opacity, which
blend to about `#a5c8e8` and `#bfbfbf`.

Chosen by computing the blend rather than by eye this time, against two
constraints: visible from a normal reading distance, and black body text still
comfortably legible on top (the focus blend keeps ~12:1 contrast with black, far
above the 4.5:1 floor). Opacity moved from `0.4` to `0.55` for both, so focus
and selection stay symmetric; the hues are unchanged in character — a blue for
focus, a neutral grey for the selection — so an existing config that only sets
one of them still reads as the same pair.

Nothing structural changed: the values live in `ViewConfig::default()`, the
fallbacks used when a user colour fails to parse were moved in step
(`syodep-ffi`, where the warning text quotes the default), and
`config/default-config.toml` was regenerated from `syodep --defaults`. Covered
by `overlay_colors_default_and_user_override`,
`default_config_doc_round_trips_to_defaults` and
`invalid_color_falls_back_and_warns`.

---

## 2026-07-29 — An abbreviation is one word and does not end a sentence

`e.g.` was four word stops and two sentences. Now abbreviations are single words
whose internal stops are inert, in the same shape as the number rule: one
predicate consulted from both `same_word_run` and `sentence_boundary_after`, so
word runs and sentence runs cannot disagree about where the construct ends.

Two kinds, deliberately handled differently. **Dotted initials** — `e.g.`,
`i.e.`, `U.S.`, `Ph.D.`, `a.k.a.` — are recognised by *shape*: a run of one- or
two-letter groups joined by stops. No list, nothing to maintain, and no sentence
ever ends in the middle of one. **Everything else** needs naming, because `Fig.`
is indistinguishable from a word ending a sentence, so there is a curated list:
Latin and citation forms, references, bibliographic forms, titles, months, days,
organisations and measurement.

### The capitalisation test, and why it is not applied generally

`etc.` and `U.S.` genuinely can end a sentence. So the closing stop of an
abbreviation still ends one — but only when a capital follows. `…oranges, etc.
The next` splits; `…etc. and then` does not.

The tempting generalisation is to apply that everywhere: "a stop followed by a
lower-case word is not an ending". Measured on the reference document first,
that rule would have merged **nine genuine sentence boundaries** — after
`match.`, `data.`, `bits.`, `rule.`, `byte.`, `type.` — because a technical
document constantly begins a sentence with a lower-case identifier. So the
capitalisation signal is consulted *only* where an abbreviation is already
suspected, which is precisely where the evidence says it is safe. A test pins
that: `an_ordinary_word_before_a_lower_case_word_still_ends_the_sentence`.

Several list entries (`no.`, `min.`, `co.`) are also ordinary words that can
close a sentence. That is safe for the same reason — the list only says "suspect
an abbreviation here", never "this is never an ending".

### Result

On the reference document, sentence stops went 415 → 413: exactly the two
spurious boundaries inside its single `e.g.`, with all nine lower-case-follows
boundaries correctly left alone.

11 new tests, 353 total.

---

## 2026-07-29 — A list item is a region, so it has an end

The first cut of list support gave items a *start* and nothing else: a
`list_starts` vector of line indices, plus two bespoke filters in the sentence
walkers refusing to cross one. That made each item a sentence but left the last
item of every list joining the prose below it, since nothing said where a list
ended.

The fix was not a `list_ends` vector beside the first. A list item is a region
like a heading or a table, and a region has two edges by construction. Giving
items an extent deletes both filters and `App::starts_list_item` outright:
`next_cell_in_region`/`prev_cell_in_region` revert character-for-character to
the one-line `same_region` filters they were before lists existed. The feature
stops existing in the motion code entirely.

`ObjectKind` grew a second predicate to pay for it. Regions now read as a chain:
a table or image is one stop at every scope; a heading is one step for `s` and
`p`; a list item is one step for `s` only, because a list is one paragraph made
of many items.

### The extent rule, and two measurements that shaped it

An item covers its marker and every following line indented past that marker,
stopping at the next marker, at any object already claimed, at a paragraph-sized
gap, or when the text returns to the marker's margin. Measured on the reference
document: markers at x0=119.6, item text and wrapped continuations at 129.5,
prose resuming at 119.6.

**A design review caught a bug that would have shipped.** On a two-column page
the first line of column two is trivially "indented past" a marker in column
one, so an item swallowed the head of the next column. A y-reset guard fixes it,
and `an_item_never_crosses_a_column_break` fails without it — verified by
removing the guard and watching the item become `(1,2)`.

**The spike then caught the guard being too strict.** An exact "did we move up?"
test cut every item off at its marker on the pages where MuPDF emits the bullet
as its own line: a bullet's box starts a point or two *below* its text's,
because the glyph is small and the text has ascenders. The guard needs a
one-line tolerance. Under-extension on the real document went from 13 items to
zero.

The remaining stops all proved to be the gap guard doing real work. In this
document the body prose sits *right* of the list markers, so the indent rule
alone can never end an item there — the reviewer predicted exactly this, and it
is why the gap guard is load-bearing rather than polish.

### Also folded in

A numbered item no longer splits at its own marker. Rather than exporting the
marker grammar a second time, the rule is *a sentence never ends inside the
first token of a list item* — exact, because detection only accepts a marker
that is followed by a space or is the whole line, so on any line it accepted the
first token **is** the marker. No character-to-cell index mapping either, which
would have diverged the moment a bullet were drawn as an image.

Detection also moved inside `content_objects`, judged against the objects
already claimed rather than against raw heading ranges. A marker inside a table
is a table row; an item reaching a figure truncates at it instead of being
discarded for touching one. Disjointness is now by construction — the scan
breaks on a blocked line — with a `debug_assert` over the sorted result.

On the reference document: 36 items, 6 tables and 27 headings, all unchanged.

18 new tests, 343 total.

---

## 2026-07-29 — A list item always starts a sentence

List items rarely end in a full stop, so a whole bulleted list — and the line
introducing it, which usually ends in a colon — read as a single sentence. `s`
now stops at the start of every item.

Extraction shapes items two ways, and both had to work. Usually the bullet and
its text are one line (`• A standard way to install…`), but where the indent is
wide enough MuPDF emits the bullet as a line of its own followed by the text as
another. Keying on "this line opens with a marker" covers both: in the split
case the bullet line is the item's start and its text line simply continues it.

Detection is shape plus corroboration. A marker counts only when at least one
other line of the same kind starts at the same left edge, which is what
separates a real list from a sentence opening `1998. That year…`. Numbered
section headings are indistinguishable from enumerated items by shape, so lines
already known to be headings are excluded outright — without that, every `2.1.
Directory layout` in the reference document became a list item. On that document
the rule finds 36 item starts, all genuine bullets, no false positives.

The two sentence-walking choke points added the check, so nothing else moved.
Lists bound sentences only: `w` still walks the marker and the words after it,
and paragraph scope still reads a list as one paragraph, which is a coherent
model — the list is the paragraph, each item a sentence within it.

**Known limit, pinned by a test:** item *starts* are boundaries and nothing
marks where a list *ends*, so the final item joins whatever prose follows it,
exactly as any unterminated line always has. Fixing it needs list extents —
knowing which lines are continuations rather than the next paragraph.
`the_last_item_runs_on_into_the_prose_after_the_list` is the test that will
change the day that lands. *(It landed the same day: see the entry above, where
items became regions with extents and that test inverted.)*

9 new tests, 323 total. New fixture: `pdf_with_list`.

---

## 2026-07-29 — A number is one word and never ends a sentence

`3.14` used to be three word stops and, worse, two sentences: the decimal point
was read as a full stop, so `s` landed in the middle of a figure and a sentence
span stopped short. Now a separator with digits on **both** sides belongs to the
number, so `3.14` and `1,234.56` are each a single word and pass through sentence
detection untouched. The rule composes, which is what makes the grouped case work
without special-casing it.

A full stop that merely follows a number is unaffected — `it costs 3.` still ends
the word and the sentence, because nothing follows the stop. That asymmetry is
the whole rule: digits on both sides, or it is punctuation as before.

Only the same line counts. A figure is not carried across a line break, and
joining one would splice text that merely happens to end and begin with digits.

Both halves come from one pure predicate, `is_inside_number(before, sep, after)`,
consulted from `same_word_run` and `sentence_boundary_after`. Abbreviations
(`Mr.`, `e.g.`) remain a known simplification — they need a dictionary, not a
shape test.

9 new tests, 314 total.

---

## 2026-07-29 — Page furniture is out of the caret's path

Moving through a paper meant stepping through the running header on every page,
the folio at the foot, any sideways stamp down the margin and the big inclined
watermark preprints carry. None of it is reading matter. It is now dropped from
the navigable content layer entirely — not skipped by motion, *removed* — so
every motion, span and overlay ignores it with no change to the caret at all.
The lines are kept in `PageContent::furniture` rather than discarded.

### Two rules

**Rotation.** A line more than 10&deg; off the page's *dominant* direction.
Dominant rather than horizontal is what lets a page laid out sideways keep
everything, and it makes the rule provably unable to empty a page: the dominant
cluster is by construction the majority and is never flagged. The angle comes
from the character quads — `ur - ul` runs along the baseline and is well-defined
even for a single glyph — as a circular mean rather than a bucketed vote, since
bucketing splits one physical direction across the ±180&deg; wraparound.

**Repetition.** A margin-band line whose digit-masked text and baseline recur
across sampled pages. Position is never evidence on its own, which is why a
paper's title survives: it appears once. Digit masking means a folio matches
itself across pages, and `Chapter 7 of 9` is correctly one running head rather
than nine headings.

### Thresholds, and the observations that set them

Measured on the shared-mime-info spec before any code was written. Its running
header sits at baseline **56.19 on 18 of 19 pages — zero jitter** — so the 2.5pt
tolerance is generous; the same text appears once more at 88.82, as the *title*
on page 0, and correctly survives. Every ordinary line reported an angle of
exactly `0.0`, so 10&deg; is enormously slack. Eight sampling passes cost 13.4ms.

Baselines, not bounding boxes, are the position key: a descender shifts a box by
points, which would make a header match on some pages and not others.

Sampling is four anchors of **two consecutive pages**, not evenly spaced singles:
100 pages sampled 8 times steps by 14 and lands on one parity, so a book that
alternates verso and recto running heads would never see the recto one repeat.

The bare-folio rule was **cut**. Digit masking already makes page numbers repeat
like anything else, and a standalone "bottom-band number" rule would have
deleted the bare-number body lines this document carries at 674–686pt. The 30%
repeat floor is load-bearing for the same reason: two body lines coincidentally
sharing a baseline score 25% and are rejected.

### Three things the tests caught

- **Table detection would have died silently.** `content_objects` discards a
  table claiming every non-empty line — a guard that passes today only because
  the header and folio pad the count. Remove them and a full-page table trips
  it. The guards now count lines *plus* furniture.
- **`image_lines` indices needed remapping.** They point into the unfiltered
  vector; dropping lines without remapping compiles cleanly and labels a line of
  text an image.
- **The upright tiebreak was unreachable.** If an upright cluster carried ≥0.9×
  the leader, the leader could never hold 55% of the page, so the branch could
  not fire. Deleted; the dominance floor already gives the safe outcome.

### One deliberate override

A design review argued a divider page holding only a header and a folio *should*
end up with zero lines, since page stepping already skips empty pages. The test
suite disagreed loudly: the shared fixture gives each page one line, `Page N
text`, which normalises identically across pages, and the whole document became
unnavigable. That is the worst failure this feature can produce — text plainly
visible that the caret cannot reach — so the repetition rule now never takes a
page's last line. Cheap insurance against a catastrophic mode, at the cost of
leaving two lines on a genuinely blank divider page.

### Known limitations

A table continued across pages whose column-header row sits at the same height
each time will be removed if it falls inside the top band; long tables usually
start lower, and a gap test cannot separate this from a real running head.
Chapter titles that change per chapter are not caught — the principled
escalation is matching band text against `Document::outline()`, which already
exists. Images are never furniture, so a logo inside a running header survives.

`Ln N` in the status line now counts body lines: the running header no longer
occupies line 1. Nothing persists it, so no migration.

23 new tests, 305 total. New fixtures: `pdf_with_running_header`,
`pdf_with_rotated_text`.

---

## 2026-07-29 — Headings are single sentence and paragraph steps

Headings rarely end in a full stop, so the sentence walker ran straight through
them and glued a section heading to the paragraph below it; paragraph scope had
the same problem whenever a heading sat close to its body text. Now a heading is
one step for `s` and `p`: `s` lands on it, the next `s` lands on the body
beneath. A numbered heading such as `2.12. Recommended checking order` counts as
one sentence rather than three, and a heading that wraps is one step across both
lines.

### Why a heading is not a table

Deliberately **not** atomic. `w` still walks a heading's individual words and
`j` at line scope still moves line by line, because a heading is ordinary prose
you may want to select a phrase of — unlike a table, which has nothing useful
inside it to traverse.

That distinction is the whole change in the core. `ObjectKind` gained
`is_atomic()`, false only for `Heading`, and the accessors that were serving two
purposes at once split in two: `atomic_object_at`/`atomic_id_at` (motion,
highlighting, snapping) exclude headings, while `region_at`/`region_id_at`
(sentence-run bounds, paragraph splitting) include them. Being a region is
already enough to be one sentence and one paragraph — `split_segments_at_objects`
and `next_cell_in_region` needed no new logic — so `step_scope_atomic` was not
touched at all. Had the new variant simply been added, every object-consuming
site was kind-agnostic and headings would have become atomic everywhere.

One extra rule: `sentence_boundary_after` ignores punctuation inside a heading,
so `2.12.` cannot split one. Tables never needed this because their atomicity
masked it.

### Detection

From typography, not structure. Body size is the character-count mode of the
page — body text dominates by volume on every page, including title pages, which
makes it far steadier than a mean or median. A line is a heading when it is
`>= 1.15x` body size, or entirely bold at body size *and* narrower than 90% of
the widest line on the page (that last clause separates a bold subsection
heading from a bold lead-in sentence). Adjacent flagged lines of equal size and
weight merge, so a wrapped title is one heading.

Both signals come free from the existing extraction pass — `TextChar::size()` is
always populated and `TextCharFlags::BOLD` is set from the font's own bold flag —
so unlike table detection there is **no second pass and no runtime cost**.

Thresholds were set against a real document rather than guessed. 1.15 rather
than 1.10 because of an observed failure: on pages dominated by 9pt code
listings, ordinary 10pt prose is 1.11x the computed body size and was being
flagged wholesale. The runaway guard is 50% rather than the tables' stricter
shape because a title page legitimately is mostly large type. On the
shared-mime-info spec the result is 27 headings — the title, the author block
and every numbered section plus `References` — with no body text flagged.

MuPDF has its own heading detection (`FZ_STEXT_PARAGRAPH_BREAK`) and it is a
dead end: it wraps headings in structure nodes whose children the Rust bindings
cannot walk, the same wall the table work hit, and it keys on bold alone.

### Headings yield to tables

Bold table column headers (`Attribute`, `Required?`, `Value`) look exactly like
bold subheadings, and on the sample document they were flagged on precisely the
six pages where tables are detected. A heading range overlapping any existing
object is dropped, which removes all of them.

### Tests

19 new tests, 268 total. Detection is a pure function tested directly against
each threshold, and the behavioural tests inject a hand-built `PageContent` with
evenly-spaced lines — spacing at which the paragraph gap heuristic alone would
merge the whole page, so the tests prove the heading edges are doing the work.
`word_motion_still_walks_through_a_heading` is the guard on the accessor split.
New fixture: `pdf_with_heading`.

---

## 2026-07-29 — Tables and images are single navigation units

Moving through a paper used to mean crawling through its tables: a table was
just many short lines, so `w`, `s` and `p` stepped through it cell by cell and
a selection could only ever grab a fragment of it. Now a table or an image is
**one unit** at every scope above char — one motion lands on it, the next lands
past it — and selecting it takes the whole thing, drawn as a single rectangle.

Char scope is deliberately left raw: `cc` then `h`/`l` still walks a table's
individual characters, so a single number in a cell stays selectable. That is
the escape hatch, and it is also why the split is at char rather than making
tables opaque everywhere.

### How it is built

- `syodep-pdf` gained `ContentObject` (a run of lines that behaves as one unit)
  and `PageContent { lines, objects }`. `page_content` now takes
  `ContentOptions`.
- Tables come from a **second** structured-text pass with
  `TABLE_HUNT | COLLECT_VECTORS`, from which only the bounding boxes are taken.
  Two passes are needed because the detection pass rewrites the page: it moves a
  table's text into a structure node the Rust bindings cannot walk into, and
  splits lines while filling cells. Its geometry is therefore unusable for text,
  and the text pass is left exactly as it was.
- `COLLECT_VECTORS` is load-bearing, not a nicety. MuPDF hunts for tables among
  a page's ruled rectangles; with no vectors collected that list is empty and it
  falls back to hunting the whole page at a loose threshold. Measured on a real
  spec document: with `TABLE_HUNT` alone, two prose pages came back as one
  page-sized "table" and nothing else was found; adding `COLLECT_VECTORS` found
  six real tables with tight boxes and left the prose pages as the only false
  positives.
- Those remaining false positives are killed by one guard: a box whose lines are
  *every* line on the page is discarded. On the same document that guard was
  exactly precise — it rejected both bad pages and no good ones. Boxes mapping
  to a non-contiguous set of lines are dropped too. Degrading to line-by-line
  navigation is always safe; a wrong atomic unit is a very visible bug.
- Cost is ~2 ms per page for the second pass, and content is already cached per
  page for the session. `view.detect_tables` (default `true`) turns it off.

### Why the motion table was not touched

`step_scope` remains the pure per-scope description of what a word, line,
sentence or paragraph is. Atomicity is one wrapper over it — `step_scope_atomic`
— which after a step keeps stepping while the caret is still inside the object
it started in, and snaps to an object's start when it lands in a new one.
Because the wrapper has the same signature, counts (`5w`) and all six call sites
work unchanged and a table costs exactly one repetition. `e` is the exception,
landing on the object's *end* so a following `e` leaves rather than walking back
through it.

Two things the wrapper cannot fix, because they are about how far a span
*reaches* rather than where a step *lands*, and both needed their own change:
paragraph segments are cut at object boundaries afterwards
(`split_segments_at_objects`), and sentence runs stop expanding at an object
edge — a table cell rarely ends in `.`, so a sentence would otherwise run
straight through the table. Searching for the next sentence still crosses
freely, which is what makes a table simply become a sentence of its own.

### Tests

19 new tests, 249 total. The mapping from boxes to line ranges is a pure
function tested directly (including every discard rule), and the motion tests
inject a hand-built `PageContent` instead of relying on the heuristic — so a
future change in MuPDF's detection can only fail the one detection test rather
than the behavioural suite. New fixture: `pdf_with_table`.

---

## 2026-07-29 — 0.8.0

Since 0.7.0. **Existing configs keep working** — unlike 0.7.0, nothing was
removed. The new `[input]` section and the new command names are additive.

- **A pause now completes a key sequence.** `c` or `v` on its own enters focus
  or visual mode after a brief stop, keeping whatever granularity is live.
  Sequences typed at normal speed are unaffected: `cw` is still word focus.
  Tunable with `[input] timeout_ms` (500 ms default, `0` disables).
- **`s` and `p` move by sentence and paragraph**, at any scope, the way `w`
  already moved by word. They are motions, not scope changes: in word focus,
  `s` jumps to the next sentence's first word and the highlight stays
  word-sized, where `cs` stays put and highlights the whole sentence.
- **Open file moved from `o` to `<C-o>`.** `o` swaps the selection ends while
  selecting, so open-file was the one command that silently had no binding in a
  mode. This is the change most likely to disturb muscle memory.
- **Returning to normal mode resets the granularity to characters.** Normal
  mode has none of its own, so it no longer remembers one: `cs`, `<Esc>`, `v`
  now starts a character selection rather than a sentence one.
- **Leaving a selection by naming a scope keeps your place.** Previously only
  `<Esc>` did; `cw` and friends dropped you back where the selection started.

### If you rebound `o`

A config that sets `"o" = "open_file"` under `[keys]` keeps that binding — user
entries extend the defaults rather than replacing them — so `o` will still open
files for you *and* `<C-o>` will too. Remove the line to follow the new default.

---

## 2026-07-29 — `s` and `p` move by sentence and paragraph

### Implemented

Focus and visual mode gain two motions: `s` to the next sentence, `p` to the
next paragraph. Like `w`/`b`/`e` they work at *every* scope and leave the scope
alone — the highlight stays whatever size the active scope makes it.

Forward only, by choice: going back a sentence is `cs` then `h`. Not bound in
normal mode, matching `w`/`e`/`b` — a motion moves the highlight, and normal
mode has none.

New commands: `focus_next_sentence`, `focus_next_paragraph`,
`visual_next_sentence`, `visual_next_paragraph`.

### Why it was mostly a deletion

`step_scope(caret, scope, dir, …)` already maps a scope and a direction onto a
motion, and its `Scope::Word` arm calls exactly the functions the word commands
called:

```rust
Scope::Word => match dir {
    Dir::Left  => self.step_prev_word_start(caret),   // == focus_prev_word
    Dir::Right => self.step_next_word_start(caret),   // == focus_next_word
```

So "move one unit of a *named* scope, ignoring the active one" already existed
— `w` and `b` were the word instance of it, written out longhand. The feature
is the sentence and paragraph instances of the same idea.

The change was therefore a generalisation: one `focus_scope_motion(scope, dir,
count)` (and its visual twin) now backs `w`, `b`, `s` and `p`. `e` is the only
leftover — it targets a word run's *end* rather than a unit's start, which no
scope motion expresses — so `WordMotion` collapsed from three variants to one
and was deleted in favour of `focus_word_end` / `visual_word_end`.

### Test strategy

The refactor half was verified by the **absence** of test changes: routing
`w`/`b` through `step_scope` left all 211 existing tests passing untouched,
which is the proof it was behaviour-preserving. Only then were the new
commands added.

Six new tests cover motion-at-every-scope (char, word and line, asserting the
highlight keeps the *active* scope's width rather than the motion's), counts,
clamping at the document end, growing a visual selection, and that a bare `s`
does not shadow the `cs` chord.

The distinguishing property — motion versus scope change — is a rendering
question no unit test answers, so it was checked under Xvfb on a
two-sentence fixture:

| Keys | Result |
|---|---|
| `cw` | "First" highlighted, word-sized |
| `s` | "Second" highlighted — next sentence, **still word-sized** |
| `cs` | same place, now "Second sentence here." — whole sentence |

The plan predicted no FFI or Qt change (commands and keymaps only);
`git diff --stat crates/syodep-ffi ui-qt` came back empty, checked rather than
assumed.

### Notes / remaining

- `s`/`p` are bare keys while `cs`/`cp` and `vs`/`vp` are two-chord sequences
  on a different trie path, so all of them keep working. `s` *moves* by a
  sentence; `cs` *focuses by* sentence.
- No backward sentence/paragraph keys. `S` and `P` are still free if that
  changes.
- Bare scope letters are dropped as an idea; this covers the motion half of
  what it was for.

---

## 2026-07-28 — A pause commits a key sequence; `o` frees up; the scope resets

Three items from the post-0.7.0 review, shipped as three commits.

### `open_file` moves to `<C-o>`

`o` opened a file in normal and focus mode but swapped the selection ends in
visual mode, so open-file was the one normal-mode command that silently had no
binding in a mode. `o` is now free everywhere and the command works in all
three modes. The empty-state status line advertised the old key, so it moved
too.

### Returning to normal mode resets the scope

Normal mode has no granularity of its own, so it cannot sensibly remember one —
but it did: `cs`, `<Esc>`, `v` started a *sentence* selection. Every path back
to normal now goes through `enter_normal_mode`, which resets the scope and
keeps the position. Leaving visual back into *focus* still carries the scope,
since focus does have a granularity worth remembering.

### A pause ends a wait

**This reverses a deliberate design decision**, which is the reason it gets its
own section. `input.rs` said in three places that the state machine was
timer-free — *"the decision is made by the next key press, never by elapsed
time"* — and `docs/keybindings.md` advertised "no timeout — behavior is fully
deterministic". Those claims are now gone rather than left contradicting the
code.

A sequence that is both a binding and a prefix (`c`, `v`, `o` while selecting)
could previously only fire by following it with an unrelated key: `vk` entered
visual mode and then moved up. Now a pause resolves it, so `v` alone works.
`InputState::timeout` reuses the existing longest-prefix walk with one change —
the range is inclusive, so the full pending sequence is itself a candidate.
That one character is the whole feature.

Two rules that are not obvious:

- **A bare count never times out.** `12`, a pause, then `G` still jumps to page
  12. Only a partial *sequence* resolves, which is why `Effects::pending_input`
  is driven by a new `has_pending_sequence()` rather than `has_pending()`.
- **A junk sequence is dropped.** A half-typed `g` clears itself instead of
  waiting indefinitely for a key that may never come.

Bare `c` needed a command to fire: `focus_enter`, which is
`enter_focus(self.focus_scope)` — no new state. Combined with the scope reset
above, that gives exactly the requested behaviour: char from normal, the live
scope from focus or visual.

**The clock lives in the shell.** `InputState::timeout` is called by a QTimer
the widget arms when `pending_input` is set; the core never reads a clock. So
the core stays deterministic, and a test "waits" by calling `timeout` directly.
New config: `[input] timeout_ms`, default 500 ms, `0` to disable.

### Test strategy

Six unit tests in `input.rs` (fire a bound prefix, keep the count, leave a bare
count alone, drop an unbound partial, inert when nothing is pending, replay
leftovers) and two app-level ones covering `c`/`v` entry per source mode and
the `pending_input` flag.

Because the timer is shell-side, none of that proves the feature works, so it
was also driven under Xvfb: `c` + wait gives a one-character highlight with no
second key; `cw` typed fast gives word focus *without* also advancing a word;
`c`, wait, `w` gives char focus and then a word motion. The middle case is the
one that matters — it is the regression a too-short pause would cause.

The new `check-docs.sh` guard for `[input]` was verified by breaking it on
purpose: renaming the documented option made the script exit 1. A guard that
has never failed is not known to guard anything.

### Notes / remaining

- `o` is now unbound in normal and focus mode. Left that way deliberately: it
  is the obvious home for a future binding.
- Drive-by: `docs/roadmap.md` still said `vl`, stale since the `e` rename.
- Bare scope letters remains blocked on `w`/`e`/`b` being word motions in both
  focus and visual mode.

---

## 2026-07-28 — The visual head *is* the focus position

### Implemented

Visual mode no longer stores its moving end. It stores only the anchored end
(`VisualAnchor`); the head is the app's `focus`/`focus_scope` — the same pair
focus mode uses. `VisualSelection` survives as a read-only view assembled by
`visual_selection()`.

Removed: `visual_goal_x`, `visual_goal_y`, `update_visual_goal_x`,
`update_visual_goal_y`, and `VisualSelection::swap_ends` (now `App::swap_visual_ends`,
which exchanges the anchor with the focus position).

Two user-visible changes fall out:

- **Leaving visual mode with a `c` chord keeps your place.** Previously
  `cw`/`ce`/… restored the position from *before* the selection started and
  discarded the head. `<Esc>` was correct; nothing else was.
- **The scope carries out too.** `cw`, `v`, `ve`, `<Esc>` now leaves you in
  *line* focus rather than reverting to word. Position and scope had been
  obeying different rules.

### Why

`self.focus` and `self.visual.head` both claimed to hold "where you are", and
were only reconciled inside `exit_visual`. Any exit that did not go through
that function read a stale value — and `enter_focus`, reached by the `c`
chords, is exactly such an exit.

This is the same failure mode the 0.7.0 collapse eliminated — derivable state
stored twice and kept in sync by discipline — surviving in the one place that
refactor did not reach. Patching the single bad read was the cheaper option and
was rejected: it fixes the symptom and leaves the trap armed for the next
caller. Deleting the duplicate makes the bug unrepresentable, which is why
`enter_focus` needed **no change at all** in the end.

### Test strategy

Both bug tests were written first and observed to fail:

```
leaving_visual_by_scope_chord_keeps_the_position
  left:  Some(Caret { page: 0, line: 0, cell: 0 })    <- pre-selection
  right: Some(Caret { page: 0, line: 0, cell: 6 })    <- the head
scope_carries_out_of_visual
  left:  (Focus, Word)    right: (Focus, Line)
```

The first version of that test exited at *line* scope, which snaps the column
to 0 — indistinguishable from the bug on a one-line fixture. It kept failing
after the fix was correct. Rewritten to exit at word scope (where the landing
cell is the head's word run) plus a two-line case for line scope, it
distinguishes the two properly. A test that cannot tell success from the bug it
targets is worse than no test.

`swapping_ends_exchanges_positions_and_scopes` moved from a `caret.rs` unit
test of the pure swap to an app-level `oo` test, since the swap now spans two
pieces of state. 202 tests pass.

The plan predicted the FFI and Qt would need no change, since `VisualSelection`
never crossed the crate boundary; `git diff --stat crates/syodep-ffi ui-qt`
came back empty, as a check rather than an assumption.

Verified visually under Xvfb: `cw` highlights the first word, `v`+`l` grows a
grey selection over two words, and `cw` then leaves the blue focus on the
*second* word — where the head was.

### Notes / remaining

- No version bump; 0.7.0 shipped hours earlier. This rides the next release.
- Still open from the same review: `o` (open file) is unreachable in visual
  mode, since `o` is swap-ends there; and `v` from normal mode inherits the
  last-used scope rather than defaulting to char.
- Bare scope letters remains blocked on `w`/`e`/`b` already being word motions
  in both modes.

---

## 2026-07-28 — 0.7.0

Since 0.6.0. **This release breaks existing configs.** See the migration below.

- **The five focus modes are now one focus mode with a scope.** `Mode` went
  from seven variants to three (`Normal`, `Focus`, `Visual`). Granularity —
  char, word, line, sentence, paragraph — is a *setting* of focus mode, not a
  mode of its own, matching how visual mode has always worked.
- **Changing granularity no longer teleports you.** Pressing `ce` while focused
  on a word now highlights the line you are on. It used to restore wherever you
  last left line focus, possibly pages away, because each mode kept its own
  mark. There is one position now, so the bug cannot occur.
- **The entry chords double as scope switches.** `cc`/`cw`/`ce`/`cs`/`cp` work
  from inside focus mode and change the scope in place.
- **`w`/`e`/`b` move by a word at every scope**, mirroring visual mode. They
  used to be bound only in caret focus, and meant scope motion in word focus.
- **The line scope is identified by `e`, not `l`** (`ce`, `ve`, `oe`). `l` is
  the forward motion in every mode and could not also name a scope.

Your keys are otherwise unchanged: `cc`/`cw`/`ce`/`cs`/`cp` still enter, `hjkl`
still move, `<Esc>` still exits.

### Migrating a config

If your config has none of the tables below, nothing to do.

The five focus key tables became one. Rename whichever you have to
`[focus_keys]`, merging them if you had more than one, and replace the command
names:

| was | now |
|---|---|
| `[caret_focus_keys]`, `[line_focus_keys]`, `[word_focus_keys]`, `[sentence_focus_keys]`, `[paragraph_focus_keys]` | `[focus_keys]` |
| `caret_focus_left`, `line_focus_left`, `word_focus_left`, `sentence_focus_prev`, `paragraph_focus_prev` | `focus_left` |
| the `*_right` / `*_next` equivalents | `focus_right` |
| the `*_up` equivalents | `focus_up` |
| the `*_down` equivalents | `focus_down` |
| `caret_focus_next_word` / `_end_word` / `_prev_word` | `focus_next_word` / `focus_end_word` / `focus_prev_word` |
| `*_focus_exit` | `focus_exit` |
| `caret_focus_enter`, `line_focus_enter`, … | `focus_enter_char`, `focus_enter_line`, … |

The motion commands dispatch on the active scope, which is why five sets
collapse to one: `focus_left` is a character in char scope, a word in word
scope, a column jump in line scope and the previous unit in sentence or
paragraph scope.

**An unmigrated config fails to load entirely**, not just its key table —
syodep rejects unknown fields so a typo cannot silently do nothing. The error
message names the stale tables and their replacement. A fresh, fully commented
reference config is at `config/default-config.toml`, and `syodep --defaults`
writes one.

### For anyone embedding the core

The C ABI changed shape. `SyoCaret`, `SyoSentence` and `SyoSelection`, plus
`syo_app_caret`, `syo_app_line`, `syo_app_word`, `syo_app_sentence`,
`syo_app_paragraph` and `syo_sentence_free`, are replaced by one `SyoOverlay`
with `syo_app_focus`, `syo_app_selection` and `syo_overlay_free`. Focus and
selection overlays are the same shape because a focus highlight is a selection
whose two ends coincide.

---

## 2026-07-28 — Five focus modes collapse into one mode with a scope

### Implemented

`Mode` went from seven variants to three: `Normal`, `Focus`, `Visual`.
Granularity is no longer a mode — `Focus` carries a `Scope` (char, word, line,
sentence, paragraph), exactly as each end of a visual selection already did.

| | before | after |
|---|---|---|
| `Mode` variants | 7 | 3 |
| Focus commands | 29 | 13 |
| Focus keymaps / config tables | 5 | 1 |
| FFI overlay getters | 5 | 1 (`syo_app_focus`) |
| Focus state fields | 8 | 5 |
| Per-mode docs pages | 5 | 1 |

The app now stores one `Caret` plus a `Scope`. What is *drawn* is derived by
`scope_span(caret, scope)` and cached in `focus_span`, mirroring how
`visual_span` has always worked. The five stored marks (`line_mark`,
`word_mark`, `sentence_mark`, `paragraph_mark`, plus three goal values) are
gone: they were derivable state the code stored and then failed to keep
consistent.

Breaking changes, all deliberate:

- `[caret_focus_keys]`, `[line_focus_keys]`, `[word_focus_keys]`,
  `[sentence_focus_keys]` and `[paragraph_focus_keys]` are **removed**,
  replaced by one `[focus_keys]`.
- The `caret_focus_*` / `line_focus_*` / `word_focus_*` / `sentence_focus_*` /
  `paragraph_focus_*` command names are **removed**, replaced by
  `focus_enter_{char,word,line,sentence,paragraph}`, `focus_exit`,
  `focus_{left,right,up,down}` and `focus_{next,prev,end}_word`.
- The status bar reads `-- FOCUS (word) --`, matching `-- VISUAL (word) --`.
- FFI: `SyoCaret`, `SyoSentence`, `SyoSelection`, `syo_app_caret`,
  `syo_app_line`, `syo_app_word`, `syo_app_sentence`, `syo_app_paragraph` and
  `syo_sentence_free` are replaced by `SyoOverlay`, `syo_app_focus`,
  `syo_app_selection` and `syo_overlay_free`.

Default *keys* are unchanged: `cc`/`cw`/`ce`/`cs`/`cp` still enter, `hjkl` still
move, `<Esc>` still exits.

### Why

A bug fixed by construction. `enter_*_focus` seeded its mark only when it was
`None`, so switching granularity landed you on a *stale* position: work in word
focus, scroll to page 30, press `ce`, and you were back wherever you last left
line focus. With one position and a scope field the bug is unrepresentable —
there is nothing to be stale. `changing_scope_keeps_the_position` covers it and
fails on the old code.

Three earlier items were symptoms of the same duplication: unifying five
hardcoded overlay colours, `Mode::CaretFocus` appearing in a dozen
enumerations that grew with every mode, and the planned bare-scope-letter
feature being self-contradictory under five modes ("stay in the mode" *is* a
mode change under the old model; under the new one it is a field assignment).

The unifying idea, now written into `docs/architecture.md`: **a focus highlight
is a selection whose two ends coincide.** That is why one `scope_span` derives
both extents, one `step_scope` table serves both motions, and one
`span_screen_rects` produces both overlays.

The entry chords doubling as scope switches falls out for free — the focus
keymap is the normal keymap plus overrides, and `c` is not overridden, so `cw`
already worked inside focus mode. It now changes the scope in place instead of
switching modes. No `focus_scope_*` commands were needed.

### Test strategy

The two de-risking commits landed first and separately: extracting the shared
`step_scope` motion table (which passed the existing suite with **zero test
edits**, converting "focus and visual already agree scope-for-scope" from an
assumption into a result — and exposing that Line scope did *not* agree), then
renaming `VisualScope` to `Scope`.

For the collapse itself, the ~100 test call sites that read the old per-scope
marks were kept working by **deriving** those shapes from `focus_span` in
`#[cfg(test)]` helpers. That is not a compatibility shim for its own sake: it
means every one of those assertions now checks what is actually drawn rather
than a parallel field, so they kept their value instead of being rewritten into
something weaker. 198 tests pass.

New tests: `changing_scope_keeps_the_position` (the bug above),
`visual_inherits_and_returns_every_focus_scope` (round-trips all five scopes
through visual and back), `word_motions_work_at_every_scope`, and
`pre_collapse_focus_tables_get_a_migration_hint`.

Verified beyond the suite: `cargo fmt`, `clippy -D warnings`,
`./scripts/check-docs.sh`, the Qt build, the offscreen smoke test, and an Xvfb
screenshot per scope confirming each highlight still renders at the right
extent in one uniform colour with no borders — plus a visual-mode capture
confirming the two overlays still differ.

### Notes / remaining

- **Old configs fail to parse entirely.** `deny_unknown_fields` rejects the
  whole file, so a config still naming `[word_focus_keys]` loses `[view]` and
  `[files]` too. The clean break stands, but the parse error now appends a
  migration hint naming the stale tables and the replacement — the bare serde
  message did not say what to do.
- **Paragraph highlights now render per line** rather than as one solid block,
  matching sentence and visual paragraph scope. The right edge is ragged where
  it used to be flush. Deliberate: it is the same span machinery for everything.
- Entering focus mode now always starts from the top-most visible line, for
  every scope. Char and line focus previously started at line 0 of the current
  page while word, sentence and paragraph used the viewport. The viewport
  behaviour is the better one and is now uniform.
- Warrants a **0.7.0** release with the breaking-change note.
- The bare-scope-letter feature (`w`/`e`/`s`/`p`/`c` switching scope in place)
  is now coherent to specify, and is the natural next step. Note the tension it
  still has to resolve: `w`, `e` and `b` are already word *motions* in both
  focus and visual mode.

---

## 2026-07-28 — Line scope is identified by `e`, not `l`

### Implemented

Renamed the *line scope identifier* in every chord:

| was | now |
|---|---|
| `cl` | `ce` — enter line focus |
| `vl` | `ve` — enter visual with line scope |
| `vl` (in visual) | `ve` — set the active end to line scope |
| `ol` (in visual) | `oe` — switch ends and set line scope |

`l` as a *motion* is untouched: it still moves forward in every mode and
scrolls right in normal mode.

### Why

Groundwork for bare scope letters (pressing `w`, `s`, `p`, … inside a focus or
visual mode to switch granularity in place). That feature needs one letter per
scope, and `l` cannot be it: `l` is the forward motion in **all seven** modes
(`scroll_right`, `caret_focus_right`, `line_focus_right`, `word_focus_right`,
`sentence_focus_next`, `paragraph_focus_next`, `visual_right`), so binding it
to a scope would gut navigation everywhere.

`e` costs far less: it is only bound in caret focus (`caret_focus_end_word`)
and visual (`visual_end_word`), and those keep working — this change touches
only the `c`-, `v`- and `o`-prefixed chords, which live on different trie paths
from the bare `e` binding.

Doing the rename first, on its own, keeps it separable from the behavioural
change that follows.

### Test strategy

No new behaviour, so no new tests: the existing suite covers the rename by
construction. The three FFI and app-level tests that drove line focus through
`c`+`l` now use `c`+`e`, and one comment that read "a single `c` is only the
first half of `cl`" was updated. `config/default-config.toml` was regenerated
from `default_config_doc()` rather than hand-edited; the diff is exactly the
five renamed bindings and the one prose line.

### Notes / remaining

- **This breaks muscle memory and existing user configs.** Anyone with `cl` in
  a `[keys]` table keeps it working — user tables extend the defaults rather
  than replacing them — but `cl` no longer enters line focus by default, and a
  config that rebinds `cl` to something else now leaves `ce` live as well.
- Historical dev-log entries still say `cl`/`vl`/`ol`. They describe what was
  true when written and are deliberately left alone.

---

## 2026-07-28 — 0.6.0

Since 0.5.0:

- **Drag and drop.** A PDF dragged onto the window opens it. Non-PDFs are
  refused while still being dragged, so nothing happens on release; dropping
  several opens the first and says how many were ignored.
- **Overlay colours are consistent and configurable.** All five focus modes
  share one colour and the selection has its own, set through
  `[view] focus_color`/`visual_color` and their opacities. No borders, and
  overlapping boxes are merged before filling so a multi-line highlight is one
  flat block instead of a banded ladder.
- **`[view] background` works.** It had been defined, documented and ignored
  since it was introduced — the shell hardcoded `#1e1e1e`.

---

## 2026-07-28 — One overlay colour per mode, configurable

### Implemented

- **All five focus modes share one colour** (`[view] focus_color`, default the
  light blue `#add8e6`); the selection uses `visual_color` (light grey
  `#d3d3d3`). Previously each mode had its own hardcoded accent — caret blue,
  line orange, word green, paragraph purple, sentence red, visual teal — so the
  highlight encoded *which scope* rather than the useful signal, focus versus
  selection.
- **No borders anywhere.** Five overlays used to draw an opaque 1px outline
  plus a fill at alpha 70 while the selection was fill-only at alpha 110, so
  two overlays with identical geometry semantics looked unrelated.
- **Overlapping boxes no longer double-blend.** Rectangles go into a
  `QPainterPath` that is `simplified()` before a single fill, which merges
  intersecting subpaths into an outline with no intersecting edges. This was a
  real defect, not a hypothetical: both multi-rect overlays take y from
  `line.bbox`, which is MuPDF's `line.bounds()` — the union of glyph quads
  including ascenders and descenders — so consecutive lines genuinely overlap,
  and the `width < 2.0` guard widens rects rightwards as well.
- **`[view] background` works again.** It was defined, documented and *dead*:
  `canvas_widget.cpp` hardcoded `#1e1e1e`, and the `setBackgroundColor()` hook
  had never been called from anywhere. Fixed with the same plumbing rather than
  left as a documented option that does nothing.
- Colour and opacity are separate options, so opacity can be tuned without
  rewriting a hex value.

### Design

Colours are resolved **once in `syo_app_new`** and cached on `SyoApp`, exactly
like `open_dir`: an unparseable value falls back to the built-in default and
reports the problem, so a typo degrades instead of producing an invisible
overlay. Opacity is clamped to `0.0..=1.0`. A new `#[repr(C)] SyoColor` and
three getters carry them to Qt — these are the **first `[view]` values ever
exposed over the FFI**; every other one is consumed inside the core.

Parsing lives in Rust rather than letting `QColor` do it, so bad values surface
through the same channel as every other config problem. `#rrggbb` only:
opacity is separate, so an eight-digit value is rejected rather than silently
reinterpreted.

The five focus getters were left as they are. Collapsing them into one
`syo_app_focus_rects()` would make the single colour structural rather than
conventional, but that is an FFI change beyond this task; the Qt-side union
gives the same visible result.

### Test strategy

Rust: `parse_hex_color` accept/reject cases, opacity clamping, config
defaults + override, and FFI tests that a configured colour reaches the getter
and that an invalid one falls back and warns.

Colour cannot be checked by exit code, so it was verified on screen under Xvfb:

- caret, word and line focus all render the *same* light blue,
- a paragraph selection is one flat grey block — sampling it gives a single
  value, `#EDEDED`, which is exactly the predicted blend of `#d3d3d3` at 0.4
  over white (211 × 0.4 + 255 × 0.6 = 237) and is identical on different
  lines. A double-blended overlap would show a second, darker value,
- a config setting `background`, `focus_color` and `visual_color` to red/blue/
  dark green takes effect for all three — the background pixel reads `#204020`,
  which was impossible before,
- `focus_color = "lightblue"` falls back to the default and shows
  `ERROR: invalid focus_color "lightblue" in [view]; using the default #add8e6`.

### Notes / remaining

- That warning lives in `last_error`, which `open_document` clears on success,
  so it is visible until a document is opened. Pre-existing behaviour shared
  with the `open_dir` warning, not introduced here.
- The default opacity of 0.4 was chosen by looking at the result, not derived.
  Pale fills with no border need more presence than the old saturated bordered
  boxes (alpha 70 ≈ 0.27).

---

## 2026-07-28 — Open a PDF dropped onto the window

### Implemented

- `MainWindow` accepts drops (`ui-qt/src/main_window.{h,cpp}`): a `.pdf`
  dragged onto the window opens it through the existing
  `MainWindow::openDocument`, which already backs the CLI argument and the
  file dialog. No new document logic, and nothing added to the core.
- Anything that is not a `.pdf` is **refused during the drag**, so the cursor
  shows "no entry" and releasing does nothing — the user finds out before
  letting go rather than getting an error afterwards.
- Dropping several PDFs opens the first and reports the rest as ignored in a
  transient status message, so the discard is visible instead of silent.

### Why it lives in the Qt shell

`AGENTS.md` says new behaviour goes in the core as a `Command`, never in Qt
event handlers. That rule is about *document/navigation behaviour*; a drop is
an OS input gesture, the same category as the mouse wheel and the CLI
argument, neither of which is a `Command` either (`wheelEvent` calls
`syo_app_scroll_by` directly; `main.cpp` calls `openDocument` directly). The
document logic was already in the core and already reachable — the handler
extracts a path and calls it.

Only `MainWindow` needed `setAcceptDrops(true)`. `CanvasWidget` is a
`QOpenGLWidget` covering the whole window, but it never enables drops, and Qt
delivers drag events to the nearest ancestor that accepts them rather than
stopping at a child that does not. That was the one assumption worth proving
rather than believing, and the test below proves it.

### Test strategy

No Rust changed, and `docs/testing.md` puts Qt widget behaviour outside the
unit-test boundary by design — so the workspace suite and `--smoke-test` only
show nothing regressed. `runSmokeTest` builds a `MainWindow` but never calls
`openDocument`, and a drop is not a key event, so neither touches this code.

`xdotool` cannot originate a drag (XDND needs a real drag *source*), so a
throwaway ~30-line Qt drag-source app was built in the scratchpad and driven
against syodep under `Xvfb`, with `xdotool` moving and releasing the mouse.
All three cases were confirmed by screenshot:

- a `.pdf` dropped on the window opens (status line `dropme.pdf [1/4] 118%`),
- a `.txt` is refused — still `no document - press 'o' to open a PDF`, and
  notably **no error**, because the drag was rejected rather than
  accepted-then-failed,
- two and three PDFs produce `Opened dropme.pdf - 1 other file ignored` and
  `- 2 other files ignored`.

That last check caught a real blemish: the message first used `tr()`'s `%n`
plural form, which needs a translation catalogue to resolve. With none loaded
`tr()` returns the source string unchanged, so users would have seen the
literal `1 other file(s) ignored`. Spelled out explicitly instead.

### Notes / remaining

- No config option to disable it; `[files]` would be the natural home, but a
  toggle for a standard gesture nobody triggers by accident is not worth the
  surface.
- No `QFileOpenEvent` handling (the macOS "open with" event) — there is no
  macOS build.
- Dropping onto the taskbar/desktop icon is file association, already covered
  by `packaging/syodep.desktop` and the Windows installer.

---

## 2026-07-28 — 0.5.0

First release carrying the Windows installer, and the first where
`--version` is trustworthy.

Since 0.4.0:

- **Visual selection mode** — a two-ended selection over the content layer.
  `v` inherits the current focus mode's granularity, `vc`/`vw`/`vl`/`vs`/`vp`
  name it, and each end carries its own scope so `o` plus a scope letter
  re-scopes the far end independently. Required teaching the input state
  machine longest-prefix fallback, which is a behaviour change in its own
  right.
- **Windows installer** — `syodep-vX.Y.Z-win64-setup.exe`: per-user, no UAC,
  silent-capable, opt-in PDF handler, and an uninstaller that leaves
  `%APPDATA%\syodep` alone because Scoop and portable installs share it.
- **One version source.** `Cargo.toml` is now the only place the version
  exists; CMake reads it and the shell reports it. 0.4.0 shipped binaries that
  told users they were 0.3.0.
- **App icon.** `syodep.exe` finally has one, plus a version resource, and the
  SVG no longer depends on a font — which also fixes the AppImage icon.
- **Release pipeline fix.** The AppImage build had been failing on a missing
  `unzip` in the container, so nothing was published between 2026-06-26 and
  2026-07-27.

Bumping `Cargo.toml` is the whole of a version bump now: `syodep --version`
reported 0.5.0 from a clean rebuild with no other file touched.

---

## 2026-07-27 — Windows NSIS installer

### Implemented

- **`packaging/syodep.nsi`**, compiled by `makensis` in `release-build-windows`
  over the `syodep-win64/` tree the staged smoke test has already validated. The
  script installs files; it never builds them.
- Per-user install to `%LOCALAPPDATA%\Programs\syodep` (no UAC), Start-menu
  shortcut, Add/Remove Programs entry including `QuietUninstallString`, silent
  `/S` install and uninstall, `/ASSOCIATE` to opt into the PDF picker.
- Attached to **tagged releases only** as `syodep-vX.Y.Z-win64-setup.exe`;
  `continuous` still carries just the zip and AppImage.
- **A tag-vs-`Cargo.toml` assertion** in the installer build: tagging `v0.5.0`
  without bumping would otherwise ship an installer and a Scoop manifest
  asserting a version the binary does not report — the same class of bug as the
  version drift fixed earlier today, one layer up.
- **The script is compiled on Linux in the `rust-lint` CI job.** `makensis` is
  cross-platform, so a broken script fails in ~1 minute rather than after the
  12-minute Windows build.

### Why the local-first workflow mattered

Installing `nsis` locally (3.10, the same version the runner has) paid for
itself immediately: the very first compile failed with

    warning 6000: unknown variable/constant "{SecAssoc}" detected, ignoring

because `.onInit` referenced the section before it was defined — NSIS resolves
`${SecAssoc}` at parse time, so the reference silently expanded to nothing and
`/ASSOCIATE` would have been a no-op. Caught in seconds; it would otherwise have
been a twelve-minute round trip, and `-WX` is what turned the warning into a
failure rather than a silently broken installer.

The same compile also revealed that **NSIS resolves a relative `OutFile` against
the script's directory, not the working directory** — the first successful build
dropped a 563 KB binary into `packaging/`. `OutFile` is now `${OUTFILE}`, passed
explicitly, with `packaging/*.exe` gitignored as a backstop.

Two more only surfaced in CI, both because the local invocation differed from
the CI one in a way that hid them.

`-DSRCDIR=syodep-win64` (relative) made makensis report *"Error while loading
icon from syodep-win64\\syodep.ico: can't open file"* — the same
script-relative resolution rule as `OutFile`, so it hunted for
`packaging/syodep-win64/`. The error names the icon, which reads like a missing
file rather than a wrong base directory. Every local test had passed an
absolute path and never exercised the relative case. All paths handed to
makensis are now absolute, and the script says so where the defines are
declared.

The other: Ubuntu's Ubuntu's
`imagemagick` package is ImageMagick **6**, whose command is `convert`, while
`magick` exists only in 7, so the lint job died with `magick: command not
found`.
`ui-qt/CMakeLists.txt` already accepted either via
`find_program(... NAMES magick convert)`; the workflow now does the same.

And running the CI lint step locally caught a third: **`makensis` aborts with
`free(): double free detected` (SIGABRT, exit 134) when `MUI_ICON` points at an
invalid `.ico`**, rather than reporting a readable error. The stub tree had
created the icon with `touch`, so it was zero bytes. The lint job now generates
a real icon from `packaging/syodep.svg`, which has the side benefit of failing
that job if the SVG ever stops rendering.

### Decisions

- **Uninstall leaves `%APPDATA%\syodep` alone.** It is shared with Scoop and
  portable installs, so wiping it would destroy reading positions belonging to a
  syodep this installer never owned — silently, via `uninstall.exe /S`. This
  reverses an earlier decision, taken before that sharing was noticed.
- **The PDF checkbox cannot claim the default handler**, and does not pretend
  to. Since Windows 8 the effective association lives in a hash-protected
  `UserChoice` key; the script registers a ProgID, `Applications\syodep.exe`
  and `OpenWithProgids` so syodep *appears* in the picker, and the wizard text
  says "Offer syodep as a PDF handler" rather than promising a default.
- **`SetErrorLevel 2` before every `Abort`.** NSIS exits 0 by default, so a
  silent install can fail invisibly — which would have made the whole CI gate
  theatre.
- **`RMDir /r "$INSTDIR"` is guarded** (path length, plus `syodep.exe` and
  `Uninstall.exe` present) because `$INSTDIR` is user-controllable through
  `/D=`. A hand-maintained file manifest was rejected: `windeployqt` emits a
  Qt-version-dependent tree that would go stale on every Qt bump.
- **Upgrades delete the stale payload in place** rather than running the old
  uninstaller. `File /r` overwrites but never removes, and a leftover Qt DLL
  from an older Qt is a startup crash with no useful message.

### Test strategy

CI-only change; no core logic touched, so no new Rust tests. The script compiles
locally against a stub tree and produces a valid PE.

The CI verification is a real gate rather than a compile check: silent install,
assert the payload and Qt plugin, assert the ARP values (including a
plausibility range on `EstimatedSize`, which catches the classic bytes-for-KiB
slip), assert the association was **not** set under a plain `/S`, run the
*installed* binary offscreen with Qt stripped from `PATH`, then uninstall and
assert everything is gone.

Three traps it is written around:

- `uninstall.exe /S` normally relaunches from `%TEMP%` and returns immediately,
  so every assertion after it races and passes only on a fast runner. `_?=`
  keeps it in place; it must be the last argument, and it means the uninstaller
  cannot delete itself.
- The "user data survives" assertion would be **vacuous** without planting a
  sentinel first: `--smoke-test` runs `syo_app_new(nullptr, nullptr)` and never
  creates `%APPDATA%\syodep`.
- A **negative test** (install into an unwritable path must exit non-zero) is
  what proves `SetErrorLevel` works. Without it every other assertion rests on
  an installer that might always exit 0. Two things had to be right for it.
  The target has to be unwritable *regardless of privilege* — the first attempt
  used `C:\Windows\System32`, which CI can write to because it runs elevated.
  And the check has to run in `.onInit`, not in a section: `Abort` in an install
  section cancels the section but the process still exits 0. Under `/S` there is
  no directory page, so `$INSTDIR` is already final at `.onInit` and can be
  rejected there.

  **The test's premise was impossible.** Logging `${GetParameters}` and
  `$INSTDIR` from `.onInit` on the runner showed `$INSTDIR` was the *default*
  install directory, not the unwritable one: **NSIS validates `/D=` and
  silently falls back to `InstallDir` when the path is unusable.** So the
  installer installed itself to its default location and correctly exited 0.
  `CheckWritable` passed because the directory it was handed genuinely was
  writable. Nothing was ever aimed at the bad path.

  There is therefore no way to provoke a failed install through `/D=`, and the
  negative test was removed rather than kept in a form that tests nothing.
  `CheckWritable` remains unexercised by CI; it is reachable only through a
  directory chosen interactively.

  Seven dispatches went into this, each testing a hypothesis about NSIS
  internals, when the fault was that the installer was never receiving the
  input the test claimed to give it. Two process errors made it worse: the
  instrumentation that settled it in one run should have gone in after the
  second failure rather than the seventh, and it was reverted once before the
  fix was confirmed, which cost another cycle. When something "does not fire",
  log its input before theorising about its logic.

  Chasing this is what motivated running the installer under **Wine** locally,
  which turned a 12-minute dispatch into a few seconds and settled three
  questions by experiment rather than by guessing: `SetErrorLevel` + `Abort` in
  `.onInit` under `/S` *does* yield exit 2; `/D=` *is* already visible in
  `.onInit`; and `CreateDirectory`/`FileOpen` do **not** reliably raise the
  error flag. So `CheckWritable` now tests outcomes — does the directory exist,
  is the handle non-empty, did the probe file actually land — rather than
  trusting the flag.

  Wine's limits are worth recording too: it happily "succeeds" at creating a
  directory beneath a regular file, so it cannot reproduce Windows filesystem
  failures. It is good for exit-code and control-flow semantics, useless for
  permission semantics.

### Notes / remaining

- **Unsigned.** SmartScreen will warn until the binary earns reputation, and
  reputation is per-publisher, so an unsigned build never accrues any. Signing
  needs an Authenticode certificate on FIPS hardware, meaning a cloud signing
  service rather than a secret in CI. Tracked as a new roadmap bullet.
- Antivirus false positives are common for unsigned NSIS installers.
- Scoop and the installer can coexist (Scoop shortcuts live under
  `Scoop Apps\`), but both appear in the picker and whichever `syodep.exe` is
  on `PATH` wins for CLI use.

---

## 2026-07-27 — App icon: font-free SVG, generated .ico, Windows resource

### Implemented

- **`packaging/syodep.svg` no longer contains text.** The Vim-style `:` was a
  `<text>` element at `font-family="monospace"`; it is now two `<circle>`s.
- **`syodep.ico` is generated, not committed** (`AGENTS.md:66` forbids checked-in
  generated binaries): the Qt build runs ImageMagick over the SVG to produce a
  7-frame icon (16/24/32/48/64/128/256).
- **`syodep.exe` now carries an icon and a VERSIONINFO block**, via
  `ui-qt/syodep.rc.in` configured by CMake. `enable_language(RC)` is called only
  under `if(WIN32)`, so non-Windows builds never look for a resource compiler.
  Version strings come from `SYODEP_VERSION`, so the file-properties dialog
  cannot disagree with `--version`.
- **Graceful degradation**: `find_program(magick)` — when ImageMagick is absent
  the whole resource is skipped with a warning and the build still works.
- The release job copies the generated icon into `syodep-win64/` and verifies
  it before shipping.

### Why

An icon built from a font glyph renders differently on every machine: the face,
weight and metrics depend on what the system resolves for `monospace`, and at
16px an unhinted glyph turns to mush. This was already live in the **AppImage**,
which ships this SVG as its icon — so the fix is not merely preparatory for
Windows.

Separately, `syodep.exe` had no icon or version resource at all, so Explorer,
the taskbar and Add/Remove Programs showed a blank generic document.

A design review claimed ImageMagick's internal renderer ignores `text-anchor`
outright, which would have meant the glyph was clipping outside the page rect.
**That could not be reproduced**: the local ImageMagick has the rsvg delegate,
renders the two anchorings differently, and `msvg:` did not bypass it. The
change was made on the ground that a shape-only icon renders identically under
every renderer — not on the unverified claim.

### Test strategy

No core logic touched, so no new Rust tests. Verified by rendering and *looking
at* the output rather than trusting exit codes: before/after at 256px, and the
16/32/48px frames pixel-magnified. At 16px the dots disappear and the icon
reduces to "document with a yellow highlight" — acceptable, and no worse than
the glyph, which was equally invisible there.

The CI blank-icon guard was validated both ways locally: the real icon scores
sd=0.387 and passes; a deliberately empty `.ico` scores sd=0 and is rejected.
A broken SVG render produces a structurally *valid* but empty icon, which a
frame count alone would not catch.

Linux builds were confirmed unaffected — a from-scratch configure and build
emits no warnings, generates no `.ico`, and produces a working binary.

### Notes / remaining

- The 16px frame is muddy: the outer rounded square plus the page inset leaves
  little room. Worth revisiting if the placeholder art is ever replaced.
- The taskbar prefers the *window* icon over the exe resource. Fully fixing the
  taskbar would need `QApplication::setWindowIcon` fed from a `.qrc`; not done.

---

## 2026-07-27 — One version source, not three

### Implemented

- `CMakeLists.txt` **reads the version out of `Cargo.toml`** at configure time
  rather than holding a copy: `file(READ)` plus a regex scoped to the
  `[workspace.package]` section (`[^[]*` stops at the next section header, so a
  dependency version can never be picked up by mistake). Pre-release suffixes
  survive for display while `project()` gets the numeric part, and
  `CMAKE_CONFIGURE_DEPENDS` on `Cargo.toml` makes a bump re-configure by itself.
- `ui-qt/CMakeLists.txt` defines `SYODEP_VERSION="${SYODEP_VERSION}"` alongside
  the existing `SYODEP_BUILD_TYPE`, and `ui-qt/src/main.cpp` passes it to
  `QApplication::setApplicationVersion` instead of a literal.
- `scripts/check-docs.sh` gained two assertions. Because drift is now impossible
  by construction, they guard the construction itself: CMake must derive the
  version rather than hardcode it, and the shell must not reintroduce a literal.
  These are the script's first content checks about packaging.
- `docs/packaging.md` "Versioning" rewritten to describe what actually happens.

### Why

The version existed in three places and two were wrong:

| Source | Was |
|---|---|
| `Cargo.toml` | 0.4.0 |
| `CMakeLists.txt` | 0.3.0 |
| `ui-qt/src/main.cpp` | `"0.3.0"` hardcoded |

The last one feeds `--version` and `--check`, so **every shipped v0.4.0 binary
told users it was 0.3.0** — and did so incoherently, since the `core:` line on
the same screen reads the Rust crate version and correctly said 0.4.0.

`docs/packaging.md` claimed CMake mirrored `Cargo.toml`. It never has: the bump
commits (`accabc8` for 0.4.0, and the same shape for 0.3.0, 0.2.0, 0.1.1) only
ever touched `Cargo.toml` and `Cargo.lock`. Nothing in CI looked at it, so the
claim and the code drifted from the first release onwards.

Found while planning the Windows installer, which has to assert a version in
its Add/Remove Programs entry and its filename — shipping an installer saying
0.4.0 over an app saying 0.3.0 was not defensible, so this landed first and on
its own.

### Test strategy

No core logic touched, so no new Rust tests. Verified by deleting `build/`,
configuring from scratch and running `syodep --version`, which prints 0.4.0 on
both the shell and core lines.

Single-sourcing was verified the only way that means anything — by editing
**only** `Cargo.toml` to `0.9.1-rc2`, rebuilding without touching any CMake
file, and confirming both lines reported `0.9.1-rc2`. That also exercised the
pre-release path, which bare `project()` would have rejected. Both new guards
were likewise confirmed to fail when deliberately broken.

### Notes / remaining

- `bucket/syodep.json` also carries a version, but CI owns it
  (`release.yml` rewrites it on every tag), so it is deliberately not covered
  by the consistency check.

---

## 2026-07-25 — Fix the AppImage release build (missing `unzip`)

### Implemented

- Added `unzip` to the apt list of the `release-build-linux` container
  (`.github/workflows/release.yml`). Nothing else changed.

### Why

The Release workflow started failing on `main` at the `Build (release)` step,
while the CI workflow stayed green:

```
command failed: unzip -q -d thirdparty/extract/src/template.odt.dir ...
sh: 1: unzip: not found
make: *** [Makethird:358: thirdparty/extract/src/odt_template.c] Error 1
```

MuPDF's `Makethird` runs `thirdparty/extract/src/docx_template_build.py` when
python3 is available, and that script shells out to `unzip` to unpack an ODT
template. The guard is `if python3 -c '...'; then <run it>; else <skip>; fi`,
so the step is skipped only when python3 is **missing** — the `unzip`
dependency is invisible right up until python3 appears.

Verified against the real image rather than guessed. In a bare `ubuntu:22.04`
neither `python3` nor `unzip` is present; simulating an install of exactly the
list this job uses shows `python3` (plus `python3-gi`, `python3-dbus`) being
pulled in transitively through apt *recommends*, while `unzip` is not. So the
container ends up with the one binary that enables the code path and without
the one that path needs.

This is why CI stayed green: `qt-build-linux` runs on a normal `ubuntu-latest`
runner, which ships `unzip` preinstalled. Only the containerized release job
has a minimal userland. The Windows release job was unaffected and passed.

The trigger was an upstream change in that recommends chain, not anything in
this repository — the previous release run (2026-06-26, `d4694a5`) succeeded
with identical workflow and lockfile content.

### Test strategy

CI-only change, no core logic touched, so no new Rust tests. Verified by
dispatching the release workflow on the fix branch and watching
`release-build-linux` reach a packaged, smoke-tested AppImage.

### Notes / remaining

- The job still depends on apt recommends for python3. That is now harmless
  either way: with `unzip` present, both branches of the guard work.
- Pinning the `ubuntu:22.04` container to a digest would make this class of
  drift impossible, at the cost of manual bumps for security updates.
  Deliberately not done here — that is a policy decision, not a bug fix.

---

## 2026-07-25 — Visual selection mode

### Implemented

- **Longest-prefix fallback in the input state machine** (`input.rs`): a
  sequence that was both a complete binding and a prefix of a longer one could
  never fire — the machine waited, and a miss discarded the whole buffer. Now a
  miss walks back to the longest pending prefix that is itself a binding, fires
  it, and queues the leftover chords for replay. Still timer-free. The queue is
  drained by `App::handle_key`, **not** inside `InputState`, because the fired
  command may change mode and the replayed chords must resolve against the new
  mode's keymap (`cw` then `vj` must run `visual_down`, not `word_focus_down`).
  `Effects::merge` keeps one key press resolving several commands from losing an
  effect bit. No-op for the previous defaults: `c`/`g`/`z` were prefixes with no
  command of their own and `o` was nobody's prefix, so every bound sequence was
  a trie leaf.
- **Visual mode** (`caret.rs`, `app.rs`): `Mode::Visual` plus `VisualScope`
  (char/word/line/sentence/paragraph) and `VisualSelection { anchor,
  anchor_scope, head, head_scope, return_mode }`. Entered with `v` — inheriting
  the scope of the mode it was entered from — or `vc`/`vl`/`vw`/`vs`/`vp` to
  name it. `<Esc>` restores the prior mode and carries its mark to the head, so
  the highlight does not snap back.
- **Two independently-scoped ends**: `v` acts on the end that is moving, `o` on
  the other one. `ow` switches ends and makes *that* end word-granular while the
  other stays as it was; `vw` re-scopes the moving end without switching. `oo`
  swaps without waiting for a motion.
- **Motion** reuses the focus modes' steppers wholesale (`step_next_word_start`,
  `line_step_down`, `sentence_step_next`, `paragraph_step_next`, …), so no new
  traversal logic was written; `w`/`b`/`e` stay word-granular in every scope.
- **Config/FFI/Qt**: a `[visual_keys]` overlay table, `syo_app_selection` /
  `syo_selection_free` returning a `SyoRect` array (modelled on the sentence
  path but with no `page` field, since a selection may span pages), and a teal
  fill-only Qt overlay.
- **Docs**: the visual-mode command page, keybinding/config references, and a
  `check-docs.sh` block for the new binding table.

### Tests

- `input.rs`: prefix fires on a miss and the leftover replays; a directly-bound
  longer sequence still wins; `5oj` gives the count to the first command while
  `o5j` replays the digit into the count branch; Escape still cancels pending
  input instead of triggering the fallback; a total miss with no bound prefix
  still resets.
- `caret.rs`: `Caret` ordering is document order; scope inheritance; swapping is
  an involution.
- `app.rs`: entry from normal and from each focus mode; growing at each scope;
  `o` is a render no-op but does move the other end afterwards; `ow` changes one
  end only; crossing the anchor and returning restores the span exactly;
  selections spanning pages; rect count bounded by the visible pages; exit
  restoring mode *and* mark; entering a focus mode dropping the selection; and
  the replay resolving against the new mode's keymap.
- FFI round-trip covers `syo_app_selection` validity, growth and the free.

### Decisions

- **Scope belongs to the endpoint, not to the start/end role.** The alternative
  (the "first" edge owns a granularity) means overshooting and coming back
  silently trades the two granularities and lands on a different selection.
  Binding scope to the endpoint makes cross-and-return the identity.
- **No `active: SelEnd` flag.** `o` swaps the anchor and head records outright,
  so the invariant is just *head moves, anchor stays* and nothing can drift.
  The one subtlety: the swap must recompute the goal column, or the next `j`
  aims at the old head's column.
- **Selections are drawn per visible page**, unlike the page-confined focus
  marks. Overlay getters are `&self` and cannot lazily extract content, so the
  resolved span is cached on `App` and refreshed after each mutation.
- **Scroll and page jumps leave the selection alone**, unlike the focus modes
  where they carry the highlight. A selection is an explicit range; dragging it
  out from under the reader would lose work.
- `v` and `o` now take effect together with the key that follows them, a direct
  consequence of the prefix design the fallback enables.

### Known limitations / next steps

- Mouse selection is still to come; roadmap phase 2 item 2 stays 🚧.
- The selection is not persisted: `Position` stores page/scroll/zoom only, so a
  restart starts in normal mode with nothing selected. Deliberate.
- Nothing consumes the selection yet — highlighting (phase 2 item 3, SQLite
  migration v2) and clipboard yank are the obvious next steps. The span is
  already exposed as `App::visual_span`.

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
