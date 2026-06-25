// Graphics/platform self-diagnosis for the syodep shell.
//
// Two responsibilities:
//   1. Pick safe graphics fallbacks before the QApplication exists, so the app
//      still starts on environments without a usable GPU path (notably WSL,
//      where the WSLg wayland-egl integration is empty and there is no
//      /dev/dxg). See decideFallbacks()/applyFallbacks().
//   2. Produce the human-readable reports for `--check` and `--version`.
//
// Detection is heuristic (platform signals), not a live GPU probe: Qt requires
// the platform plugin and software-GL choice to be made *before* QApplication
// is constructed, so there is no context to probe yet at decision time.
#pragma once

#include <QString>

namespace syodep::diag {

struct PlatformInfo
{
    QString osName;             // "Windows" / "Linux" / "macOS" / "Unknown"
    bool isWsl = false;         // running under WSL
    QString wslSignal;          // how WSL was detected (for the report)
    bool hasDxg = false;        // /dev/dxg present (WSL GPU passthrough)
    bool hasDisplay = false;    // X11 DISPLAY set
    QString displayValue;
    bool hasWaylandDisplay = false; // WAYLAND_DISPLAY set
    QString waylandValue;
};

struct GraphicsDecision
{
    bool userOverride = false;  // user set the env themselves; we touch nothing
    QString overrideReason;
    bool forcePlatform = false; // set QT_QPA_PLATFORM
    QString platform;           // e.g. "xcb"
    QString platformReason;
    bool forceSoftwareGl = false; // set Qt::AA_UseSoftwareOpenGL
    QString softwareReason;
};

struct GlProbe
{
    bool ok = false;
    QString renderer;
    QString version;
    QString vendor;
    QString error;
};

// Detect the host platform from env and filesystem signals.
PlatformInfo detectPlatform();

// Pure decision: given the platform, what (if anything) to override.
GraphicsDecision decideFallbacks(const PlatformInfo &info);

// Apply the decision. MUST be called before constructing QApplication.
void applyFallbacks(const GraphicsDecision &decision);

// Create a throwaway offscreen GL context and read its strings. Requires a
// constructed QApplication.
GlProbe probeOpenGl();

// Multi-line report for `syodep --check`. Requires a constructed QApplication.
QString buildCheckReport(const PlatformInfo &info, const GraphicsDecision &decision);

// Multi-line report for `syodep --version`.
QString buildVersionReport(const PlatformInfo &info);

} // namespace syodep::diag
