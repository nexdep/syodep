# Why successful Wayland startup should be quiet

This document explains the change in pull request #17. It starts with the
basic terms, describes the two startup warnings that motivated the work, and
then walks through the implementation and tests. No knowledge of Linux
graphics or Qt is assumed.

## The short version

Syodep was starting successfully on WSLg, but successful startup could print
messages that looked like serious graphics failures. There were two separate
causes:

1. Mesa could try an unavailable graphics route called Zink, report that the
   attempt failed, and then successfully use a different route.
2. Qt 6.2 could ask a Wayland compositor to activate a new fullscreen window,
   even though Wayland deliberately leaves that decision to the compositor.

Neither message meant that Syodep had failed. Even so, alarming output during
a successful launch makes it difficult to tell a real failure from a harmless
fallback.

The fix does three things:

- it prevents the unsupported fullscreen activation request;
- it temporarily captures startup diagnostics and removes only a small list of
  known fallback messages, and only when startup succeeds;
- it strengthens the automated smoke tests so either renderer fails the test
  if those messages escape again.

Real startup failures and unrelated diagnostics are still printed unchanged.

## Terms used in this document

### Linux display system

An application does not normally draw directly onto the physical screen. It
asks a display system to create windows and present their contents.

**Wayland** is the display protocol supported by the Linux build of Syodep.
Syodep intentionally does not fall back to X11.

A **Wayland compositor** is the program that owns the desktop and decides where
windows appear, which window receives keyboard focus, and how completed frames
reach the display.

**WSL** means Windows Subsystem for Linux. It lets Linux programs run on
Windows.

**WSLg** is the graphical part of WSL. It provides a Wayland compositor so a
Linux graphical application can display a window on the Windows desktop. From
Syodep's point of view, WSLg is an ordinary Wayland compositor; the application
does not contain a special WSL code path.

### Qt and window mapping

**Qt** is the C++ user-interface toolkit used by Syodep's Linux and Windows
shell. Qt creates the application window, receives input, and provides the
painting surfaces used by both renderers.

The **Qt Wayland plugin** translates Qt window operations into Wayland protocol
requests.

A **top-level window** is a normal application window managed directly by the
desktop, rather than a child control such as a button or list.

**Mapping a window** means making that window visible to the compositor for the
first time. A large amount of display initialization can happen lazily at this
point. In other words, constructing a window object does not prove that every
graphics component needed to show it has already been initialized.

**Fullscreen** means that the top-level window asks to occupy the complete
display area instead of opening as an ordinary movable window.

**Activation** is the request for a window to become the active or focused
window. Wayland gives the compositor final control of focus so applications
cannot simply take it whenever they want.

### Rendering and graphics libraries

A **renderer** is the implementation that turns a page and the user-interface
overlays into pixels.

Syodep has two renderer choices:

- **OpenGL** uses a graphics API and a GPU driver, or a software implementation
  of that API, to present the canvas.
- **Raster** paints the canvas through the CPU-backed Qt widget path. It avoids
  making an OpenGL canvas a requirement.

The two renderers share the same page and overlay painting logic. The renderer
choice changes how the finished pixels are presented, not what a PDF means or
how navigation works.

When `--renderer=auto` is used, Syodep performs an **OpenGL probe**. The probe
creates a tiny real OpenGL widget and paints a complete frame. If that succeeds,
Syodep selects OpenGL. If it fails, Syodep selects raster. The explicit
`--renderer=opengl` option requires the probe to succeed, while
`--renderer=raster` skips the probe.

**Mesa** is the open-source graphics implementation commonly used on Linux.

**EGL** is an interface used to connect a rendering API such as OpenGL to the
native display system. **libEGL** is the library that implements that interface.

**Vulkan** is another low-level graphics API.

**Zink** is a Mesa driver that implements OpenGL on top of Vulkan. If Zink is
available but cannot find a usable Vulkan device, Mesa can reject that route
and continue looking for another usable route.

A **fallback** is that move from an unsuccessful attempt to a different
implementation that does work. A failed attempt is not the same thing as a
failed application when a later fallback completes successfully.

### Diagnostics and testing

Programs conventionally have two text output streams:

- **standard output**, or stdout, for normal output;
- **standard error**, or stderr, for diagnostics.

On Unix-like systems each open stream has a small integer called a
**file descriptor**. Stderr is file descriptor 2. Mesa and libEGL can write
directly to this descriptor, which means their messages do not necessarily go
through Qt's logging system.

A **warning** reports a suspicious or unsupported operation but does not always
mean the program must stop. An **error message** can likewise describe one
failed attempt inside a successful fallback sequence. Whether the complete
operation succeeded is more important than a word in one intermediate line.

An **allow-list** is an explicit list of messages that may receive special
treatment. Anything not on the list is preserved.

A **smoke test** launches enough of the real application to prove that its main
pieces work together. Syodep's smoke test opens a generated PDF, renders it,
maps its real main window, and exercises important UI paths before exiting.

**CI**, or continuous integration, runs automated builds and tests for every
relevant branch and pull request. This repository uses GitHub Actions for CI.

`QT_FATAL_WARNINGS=1` is a Qt setting that turns a Qt warning into a process
failure. It is useful in CI because a warning cannot quietly become accepted
behavior. Mesa and libEGL messages are not necessarily Qt warnings, so CI also
searches the captured process output for their known prefixes.

An **AppImage** is the portable Linux package published by Syodep. It contains
the application and the Qt libraries/plugins needed to run it on supported
Linux distributions. AppImage testing matters because the bundled Qt version
and plugin set can behave differently from the versions installed on a
developer's machine.

## What users saw before this change

The application window appeared and worked, but the continuous AppImage could
print output similar to the following during startup on WSLg:

```text
libEGL warning: ...
MESA: error: ZINK: failed to choose pdev
qt.qpa.wayland: Wayland does not support QWindow::requestActivate()
```

The exact set of lines depended on the selected renderer and how the first
window was shown.

These lines were easy to misread:

- `error` suggested that rendering had failed completely;
- the message mentioned low-level graphics components unfamiliar to most
  users;
- it appeared during every successful launch, making real new diagnostics
  harder to notice.

Silencing all stderr output would have hidden genuine failures. The goal was
therefore narrower: make a successful, understood fallback quiet without
discarding evidence when the overall operation fails.

## Cause 1: raster startup can still initialize Wayland-EGL

It is natural to assume that raster mode never touches EGL because it does not
create Syodep's OpenGL canvas. That assumption is too broad.

Qt chooses and initializes several platform integrations of its own. Some of
that work is delayed until the first top-level window is mapped. The AppImage's
available Wayland client-buffer integration could try the Wayland-EGL path even
for a raster-backed window. Mesa then tried Zink, found no usable Vulkan
physical device, and printed libEGL/Mesa diagnostics. Qt successfully fell back
to its shared-memory backing store and the raster window appeared normally.

The important sequence was:

1. Syodep selected raster and correctly skipped its OpenGL probe.
2. Syodep constructed the main window.
3. The first visible top-level window caused lazy Qt Wayland initialization.
4. Wayland-EGL tried Zink.
5. Zink could not use a Vulkan device and printed diagnostics.
6. Qt used shared memory instead.
7. The window worked.

The existing stderr capture surrounded only step 1's OpenGL probe. It had
already ended before the later window-mapping attempt, so it could not classify
the mapping diagnostics according to the mapping result.

## Cause 2: fullscreen startup requested activation

The default configuration starts the main window fullscreen. Qt 6.2 followed
`showFullScreen()` with `QWindow::requestActivate()`.

That activation request is unsupported by Qt's Wayland plugin because the
Wayland compositor owns the focus decision. The compositor was already free to
focus the newly mapped window, so rejecting the explicit request did not stop
the window from working. It only produced a warning.

Windowed startup did not take the same path and remained quiet.

Qt provides the `WA_ShowWithoutActivating` window attribute. Setting it before
the fullscreen window is mapped tells Qt not to issue the unsupported
activation request. This matches Wayland's focus model: the compositor still
decides whether the new window receives focus.

## Requirements that shaped the solution

The implementation follows these safety rules:

1. **Do not hide a failed startup.** If the bounded operation fails, replay
   every captured byte exactly as it was received.
2. **Do not hide unrelated diagnostics.** A successful operation removes only
   known fallback lines; every other line is replayed.
3. **Do not suppress stderr for the whole session.** Capture only the short
   probe or first-mapping operation whose success can be checked.
4. **Do not introduce WSL-specific behavior.** The same Linux Wayland logic
   applies to WSLg and native compositors.
5. **Do not make raster depend on OpenGL.** Raster still skips Syodep's OpenGL
   probe; this change only handles Qt's later platform initialization.
6. **Keep production and smoke-test startup aligned.** The smoke test must use
   the same fullscreen helper as the normal application.
7. **Test the packaged application too.** The AppImage smoke run must enforce
   the same clean-startup rule as the ordinary Linux CI build.

## How the bounded stderr capture works

The old `ProbeStderrCapture` was private to the OpenGL probe. It is replaced by
the reusable `FallbackStderrCapture` declared in `ui-qt/src/diagnostics.h` and
implemented in `ui-qt/src/diagnostics.cpp`.

On Linux, one capture follows this sequence:

1. Open a temporary file.
2. Flush stderr so earlier text cannot be mistaken for part of this operation.
3. Duplicate file descriptor 2 and keep the duplicate as the saved real
   stderr.
4. Redirect file descriptor 2 to the temporary file.
5. Perform one bounded operation, such as the OpenGL probe or first mapping.
6. Flush again, restore the saved stderr descriptor, and close the duplicate.
7. Read everything captured in the temporary file.
8. Replay either all of it or a filtered version, depending on whether the
   operation succeeded.

The Unix `dup` call performs step 3. The `dup2` call performs the redirection
and later restoration. This descriptor-level approach is necessary because
Mesa and libEGL can write directly to stderr instead of using a Qt logger.

The capture is designed to fail safely:

- If the temporary file or descriptor setup fails, stderr was never fully
  redirected, so diagnostics continue going to the real stderr.
- Calling `finish(false)` replays every captured byte.
- If code leaves the scope without calling `finish`, the destructor treats the
  operation as unsuccessful and replays everything.
- Restoring stderr happens before reading and replaying the captured text, so
  replay cannot be captured recursively.

On non-Linux platforms the capture is inactive. This change is about the Linux
Wayland startup path and does not alter Windows diagnostics.

## Exactly which lines may be removed

After a successful bounded operation, the capture removes only:

- blank lines;
- lines beginning with `libEGL warning:`;
- the exact Mesa line
  `MESA: error: ZINK: failed to choose pdev`;
- the exact Qt line
  `qt.qpa.wayland: Wayland does not support QWindow::requestActivate()`.

All other output is replayed to stderr.

The Mesa and Qt matches are deliberately specific. A new Mesa error, a
different Qt warning, a Syodep diagnostic, or any other unexpected output stays
visible. The `libEGL warning:` prefix is used because libEGL includes additional
driver-specific detail after that stable prefix.

The operation result controls filtering. The same known line is kept when the
operation fails, because in that context it may help explain the failure.

## How the application startup path changed

### One helper shows the main window

`ui-qt/src/main.cpp` now has a `showMainWindow` helper used by both normal
startup and the smoke test.

For an ordinary window, the helper calls `show()` as before.

For a fullscreen window on Linux, it first sets
`Qt::WA_ShowWithoutActivating`, then calls `showFullScreen()`. Other platforms
continue to call `showFullScreen()` without the Linux-specific attribute.

Factoring this into one helper is important. If production and smoke tests used
different window-showing code, the test could pass while the real fullscreen
launch still printed the warning.

### The first real mapping is captured

Normal startup now begins a `FallbackStderrCapture` after the renderer decision
and before construction/mapping of the real main window. Syodep constructs the
window, optionally opens the requested document, shows the window, and processes
pending Qt events while the capture is active.

Processing events matters because mapping is asynchronous from the
application's point of view. Calling `show()` requests visibility, while
`QApplication::processEvents()` gives Qt an opportunity to perform the actual
Wayland work before the capture is finished.

`window.isVisible()` is then used as the success result. A visible window gets
the narrow successful-fallback filter. A window that did not become visible
causes every captured diagnostic to be replayed.

### The OpenGL probe keeps its previous protection

The OpenGL probe now uses `FallbackStderrCapture` instead of the old
probe-specific class. Its behavior is otherwise preserved: known failed-driver
attempts are quiet only after the probe successfully paints a complete frame.
An unsuccessful probe replays its full diagnostic output.

## How the smoke test changed

The smoke test has two first-mapping points worth controlling:

1. It temporarily maps a standalone annotation sidebar before the main window.
   That can be the operation that first initializes the raster Wayland client.
2. It later maps the real `MainWindow`.

Each mapping now has its own bounded `FallbackStderrCapture`. The standalone
sidebar succeeds when it becomes visible. The main-window section calls the
same `showMainWindow` helper as production and succeeds when that window becomes
visible.

This does more than remove output from the test. It ensures the test exercises
the production fullscreen path instead of its previous windowed-only shortcut.

## How CI prevents the regression

The Linux build workflow and the final AppImage workflow now run the smoke test
once with `--renderer=opengl` and once with `--renderer=raster`.

For each renderer, CI:

1. sets `QT_FATAL_WARNINGS=1`;
2. runs the complete smoke test;
3. captures stdout and stderr together;
4. prints the captured output so the job log remains useful;
5. searches for `libEGL warning:`, `MESA: error: ZINK:`, or
   `requestActivate`;
6. fails if any of those patterns are present.

The two protections cover different sources:

- `QT_FATAL_WARNINGS=1` catches Qt warnings by turning them into failures;
- the text search catches Mesa/libEGL output written outside Qt's warning
  system and also makes the targeted policy explicit.

The CI search is intentionally broader than the application's removal
allow-list. For example, the application filters one exact known Zink failure,
while CI rejects any leaked line beginning with `MESA: error: ZINK:`. If a new
Zink message appears, the build fails and requires a deliberate decision; it
does not silently expand what the application hides.

The AppImage check is essential because it exercises the bundled Qt 6.2
runtime that originally issued the fullscreen activation warning.

## File-by-file summary

| File | Why it changed |
| --- | --- |
| `ui-qt/src/diagnostics.h` | Declares the reusable bounded stderr-capture class. |
| `ui-qt/src/diagnostics.cpp` | Implements descriptor redirection, success-dependent filtering, full replay on failure, and reuse by the OpenGL probe. |
| `ui-qt/src/main.cpp` | Shares real fullscreen mapping between production and smoke tests, prevents unsupported Linux activation, and captures first-window mapping. |
| `.github/workflows/ci.yml` | Runs both renderers with fatal Qt warnings and rejects leaked fallback-warning patterns. |
| `.github/workflows/appimage.yml` | Applies the same rule to the final packaged AppImage. |
| `docs/architecture.md` | Records lazy Wayland initialization and the narrow successful-mapping capture as part of the renderer architecture. |
| `docs/development-log.md` | Records the symptoms, design decision, and test strategy in project history. |

## What this change does not do

This change does not:

- make Linux Syodep support X11;
- add a WSL or WSLg special case;
- remove the OpenGL probe;
- make raster rendering use OpenGL;
- hide all Mesa, EGL, Qt, or Syodep diagnostics;
- change PDF rendering, navigation, selection, highlighting, or persistence;
- force a newly shown window to take focus;
- treat a failed startup as successful.

## Expected behavior after the change

For a successful launch:

- OpenGL mode opens normally and does not leak the targeted fallback warnings.
- Raster mode opens normally and does not leak the targeted fallback warnings.
- Fullscreen startup lets the Wayland compositor decide focus without Qt making
  the unsupported activation request.
- Unrelated diagnostics remain visible.

For a failed probe or failed first mapping:

- every captured diagnostic is replayed;
- the failure remains visible to the user and CI;
- no allow-listed line is removed merely because it is familiar.

That distinction is the central rule of the change: a successful fallback may
be quiet, but a failed operation must explain itself.
