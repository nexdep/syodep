// Graphics/platform startup policy and human-readable diagnostics.
//
// Linux is deliberately Wayland-only. Renderer selection is capability-based
// and contains no WSL/WSLg detection: a working WSLg instance is simply a
// Wayland compositor like any other from the application's point of view.
#pragma once

#include <QTemporaryFile>
#include <QString>

#include "canvas_widget.h"

namespace syodep::diag {

struct PlatformInfo
{
    QString osName;
    QString waylandDisplay;
};

enum class RendererPreference
{
    Auto,
    OpenGl,
    Raster,
};

struct GlProbe
{
    bool attempted = false;
    bool ok = false;
    QString renderer;
    QString version;
    QString vendor;
    QString error;
};

struct RendererDecision
{
    RendererPreference requested = RendererPreference::Auto;
    RendererBackend selected = RendererBackend::Raster;
    bool usable = true;
    bool fellBack = false;
    QString reason;
};

// Mesa/libEGL and old Qt Wayland plugins can print recoverable startup
// attempts directly to stderr. Capture only the bounded operation and discard
// a narrow allow-list after it succeeds; failures replay every diagnostic.
class FallbackStderrCapture final
{
public:
    FallbackStderrCapture();
    ~FallbackStderrCapture();

    FallbackStderrCapture(const FallbackStderrCapture &) = delete;
    FallbackStderrCapture &operator=(const FallbackStderrCapture &) = delete;

    void finish(bool operationSucceeded);

private:
    QTemporaryFile m_file;
    int m_savedFd = -1;
    bool m_active = false;
};

PlatformInfo detectPlatform();

// On Linux, accept only the generic `wayland` QPA and select it when unset.
// Returns false with a user-facing error for every X11/offscreen override.
bool configurePlatform(int argc, char *argv[], QString *error);

RendererPreference parseRendererPreference(const QString &value, bool *ok);
QString rendererPreferenceName(RendererPreference preference);
QString rendererBackendName(RendererBackend backend);

// Show a tiny real QOpenGLWidget and require one composited frame. This catches
// failures that an offscreen QOpenGLContext alone cannot predict.
GlProbe probeOpenGlWidget(int timeoutMs = 1200);
RendererDecision decideRenderer(RendererPreference preference, const GlProbe &probe);

QString buildCheckReport(const PlatformInfo &info,
                         const RendererDecision &decision,
                         const GlProbe &probe);
QString buildVersionReport(const PlatformInfo &info);

} // namespace syodep::diag
