# Development log

Newest entries first. Each entry records what was implemented, the tests
that cover it, and decisions worth remembering. Future contributors (human
or agent): read `docs/architecture.md` first, then the latest entries here,
then `docs/roadmap.md` for what to build next.

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
