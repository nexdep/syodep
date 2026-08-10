#include "diagnostics.h"

#include <QByteArray>
#include <QCoreApplication>
#include <QEventLoop>
#include <QFileInfo>
#include <QGuiApplication>
#include <QOpenGLContext>
#include <QOpenGLFunctions>
#include <QOpenGLWidget>
#include <QTemporaryFile>
#include <QTimer>
#include <QtGlobal>

#include <cstdio>

#if defined(Q_OS_LINUX)
#include <unistd.h>
#endif

#include "syodep_ffi.h"

#ifndef SYODEP_BUILD_TYPE
#define SYODEP_BUILD_TYPE "unknown"
#endif

namespace syodep::diag {

namespace {

QString takeSyoString(char *s)
{
    if (!s)
        return {};
    const QString out = QString::fromUtf8(s);
    syo_string_free(s);
    return out;
}

class ProbeWidget final : public QOpenGLWidget
{
public:
    GlProbe result;
    bool painted = false;

protected:
    void initializeGL() override
    {
        result.attempted = true;
        QOpenGLContext *ctx = context();
        if (!ctx || !ctx->isValid()) {
            result.error = QStringLiteral("QOpenGLWidget context is invalid");
            return;
        }
        QOpenGLFunctions *functions = ctx->functions();
        const auto string = [functions](GLenum name) {
            const GLubyte *value = functions->glGetString(name);
            return value
                ? QString::fromUtf8(reinterpret_cast<const char *>(value))
                : QString();
        };
        result.renderer = string(GL_RENDERER);
        result.version = string(GL_VERSION);
        result.vendor = string(GL_VENDOR);
    }

    void paintGL() override { painted = true; }
};

#if defined(Q_OS_LINUX)
QByteArray withoutSuccessfulFallbackNoise(const QByteArray &captured)
{
    QByteArray replay;
    const QList<QByteArray> lines = captured.split('\n');
    for (const QByteArray &line : lines) {
        const QByteArray trimmed = line.trimmed();
        const bool failedDriverAttempt = trimmed.isEmpty()
            || trimmed.startsWith("libEGL warning:")
            || trimmed == "MESA: error: ZINK: failed to choose pdev"
            || trimmed == "qt.qpa.wayland: Wayland does not support "
                          "QWindow::requestActivate()";
        if (!failedDriverAttempt) {
            replay.append(line);
            replay.append('\n');
        }
    }
    return replay;
}
#endif

bool isPlatformArgument(const QString &arg)
{
    return arg == QStringLiteral("-platform");
}

} // namespace

FallbackStderrCapture::FallbackStderrCapture()
{
#if defined(Q_OS_LINUX)
    if (!m_file.open())
        return;
    std::fflush(stderr);
    m_savedFd = ::dup(STDERR_FILENO);
    if (m_savedFd < 0)
        return;
    if (::dup2(m_file.handle(), STDERR_FILENO) < 0) {
        ::close(m_savedFd);
        m_savedFd = -1;
        return;
    }
    m_active = true;
#endif
}

FallbackStderrCapture::~FallbackStderrCapture()
{
    finish(false);
}

void FallbackStderrCapture::finish(bool operationSucceeded)
{
#if defined(Q_OS_LINUX)
    if (!m_active)
        return;

    std::fflush(stderr);
    ::dup2(m_savedFd, STDERR_FILENO);
    ::close(m_savedFd);
    m_savedFd = -1;
    m_active = false;

    if (m_file.seek(0)) {
        const QByteArray captured = m_file.readAll();
        const QByteArray replay = operationSucceeded
            ? withoutSuccessfulFallbackNoise(captured)
            : captured;
        if (!replay.isEmpty()) {
            std::fwrite(replay.constData(), 1,
                        static_cast<size_t>(replay.size()), stderr);
            std::fflush(stderr);
        }
    }
#else
    Q_UNUSED(operationSucceeded);
#endif
}

PlatformInfo detectPlatform()
{
    PlatformInfo info;
#if defined(Q_OS_WIN)
    info.osName = QStringLiteral("Windows");
#elif defined(Q_OS_MACOS)
    info.osName = QStringLiteral("macOS");
#elif defined(Q_OS_LINUX)
    info.osName = QStringLiteral("Linux");
#else
    info.osName = QStringLiteral("Unknown");
#endif
    info.waylandDisplay = qEnvironmentVariable("WAYLAND_DISPLAY");
    return info;
}

bool configurePlatform(int argc, char *argv[], QString *error)
{
#if defined(Q_OS_LINUX)
    QString requested = qEnvironmentVariable("QT_QPA_PLATFORM");
    for (int i = 1; i < argc; ++i) {
        const QString arg = QString::fromLocal8Bit(argv[i]);
        if (isPlatformArgument(arg)) {
            if (i + 1 >= argc) {
                if (error)
                    *error = QStringLiteral("%1 requires a value; Linux syodep supports only '-platform wayland'").arg(arg);
                return false;
            }
            requested = QString::fromLocal8Bit(argv[++i]);
        }
    }

    if (!requested.isEmpty() && requested != QStringLiteral("wayland")) {
        if (error) {
            *error = QStringLiteral(
                         "Linux syodep is Wayland-only; requested Qt platform '%1' is unsupported")
                         .arg(requested);
        }
        return false;
    }
    qputenv("QT_QPA_PLATFORM", QByteArrayLiteral("wayland"));
#else
    Q_UNUSED(argc);
    Q_UNUSED(argv);
    Q_UNUSED(error);
#endif
    return true;
}

RendererPreference parseRendererPreference(const QString &value, bool *ok)
{
    const QString normalized = value.trimmed().toLower();
    if (normalized == QStringLiteral("auto")) {
        if (ok)
            *ok = true;
        return RendererPreference::Auto;
    }
    if (normalized == QStringLiteral("opengl")) {
        if (ok)
            *ok = true;
        return RendererPreference::OpenGl;
    }
    if (normalized == QStringLiteral("raster")) {
        if (ok)
            *ok = true;
        return RendererPreference::Raster;
    }
    if (ok)
        *ok = false;
    return RendererPreference::Auto;
}

QString rendererPreferenceName(RendererPreference preference)
{
    switch (preference) {
    case RendererPreference::Auto:
        return QStringLiteral("auto");
    case RendererPreference::OpenGl:
        return QStringLiteral("opengl");
    case RendererPreference::Raster:
        return QStringLiteral("raster");
    }
    return QStringLiteral("unknown");
}

QString rendererBackendName(RendererBackend backend)
{
    return backend == RendererBackend::OpenGl
        ? QStringLiteral("opengl")
        : QStringLiteral("raster");
}

GlProbe probeOpenGlWidget(int timeoutMs)
{
#if defined(Q_OS_LINUX)
    FallbackStderrCapture probeStderr;
#endif
    GlProbe result;
    {
        ProbeWidget widget;
        widget.result.attempted = true;
        widget.setWindowFlags(Qt::Tool | Qt::FramelessWindowHint);
        widget.setAttribute(Qt::WA_ShowWithoutActivating);
        widget.resize(1, 1);

        QEventLoop loop;
        QTimer timeout;
        bool frameSwapped = false;
        timeout.setSingleShot(true);
        QObject::connect(&timeout, &QTimer::timeout, &loop, &QEventLoop::quit);
        QObject::connect(&widget, &QOpenGLWidget::frameSwapped,
                         &loop, [&]() {
                             frameSwapped = true;
                             loop.quit();
                         });
        timeout.start(timeoutMs);
        widget.show();
        loop.exec();

        const bool frameCompleted = widget.isValid() && widget.painted && frameSwapped;
        widget.hide();
        QCoreApplication::processEvents();

        if (!frameCompleted) {
            if (widget.result.error.isEmpty()) {
                widget.result.error = widget.isValid()
                    ? QStringLiteral("timed out waiting for the first composited OpenGL frame")
                    : QStringLiteral("QOpenGLWidget failed to create a valid context");
            }
        } else if (widget.result.renderer.isEmpty()) {
            widget.result.error = QStringLiteral("OpenGL context returned no renderer");
        } else {
            widget.result.ok = true;
        }
        result = widget.result;
    }
    QCoreApplication::processEvents();
#if defined(Q_OS_LINUX)
    probeStderr.finish(result.ok);
#endif
    return result;
}

RendererDecision decideRenderer(RendererPreference preference, const GlProbe &probe)
{
    RendererDecision decision;
    decision.requested = preference;

    if (preference == RendererPreference::Raster) {
        decision.selected = RendererBackend::Raster;
        decision.reason = QStringLiteral("raster renderer requested");
        return decision;
    }
    if (probe.ok) {
        decision.selected = RendererBackend::OpenGl;
        decision.reason = QStringLiteral("OpenGL probe succeeded");
        return decision;
    }
    if (preference == RendererPreference::OpenGl) {
        decision.selected = RendererBackend::OpenGl;
        decision.usable = false;
        decision.reason = probe.error;
        return decision;
    }

    decision.selected = RendererBackend::Raster;
    decision.fellBack = true;
    decision.reason = probe.error.isEmpty()
        ? QStringLiteral("OpenGL probe failed")
        : probe.error;
    return decision;
}

QString buildCheckReport(const PlatformInfo &info,
                         const RendererDecision &decision,
                         const GlProbe &probe)
{
    QString out;
    const auto line = [&out](const QString &value) { out += value + QLatin1Char('\n'); };

    line(QStringLiteral("syodep --check"));
    line({});
    line(QStringLiteral("Platform"));
    line(QStringLiteral("  OS:               %1").arg(info.osName));
#if defined(Q_OS_LINUX)
    line(QStringLiteral("  Wayland display:  %1")
             .arg(info.waylandDisplay.isEmpty()
                      ? QStringLiteral("default (WAYLAND_DISPLAY unset)")
                      : info.waylandDisplay));
#endif
    line({});

    line(QStringLiteral("Graphics"));
    line(QStringLiteral("  Qt platform:      %1").arg(QGuiApplication::platformName()));
    line(QStringLiteral("  Requested:        %1")
             .arg(rendererPreferenceName(decision.requested)));
    line(QStringLiteral("  Selected:         %1")
             .arg(rendererBackendName(decision.selected)));
    if (probe.attempted) {
        line(QStringLiteral("  OpenGL probe:     %1")
                 .arg(probe.ok ? QStringLiteral("OK")
                               : QStringLiteral("FAILED (%1)").arg(probe.error)));
        if (probe.ok) {
            line(QStringLiteral("  GL renderer:      %1").arg(probe.renderer));
            line(QStringLiteral("  GL vendor:        %1").arg(probe.vendor));
            line(QStringLiteral("  GL version:       %1").arg(probe.version));
        }
    } else {
        line(QStringLiteral("  OpenGL probe:     skipped"));
    }
    if (decision.fellBack)
        line(QStringLiteral("  Fallback:         %1").arg(decision.reason));
    line({});

    line(QStringLiteral("Configuration"));
    const QString configPath = takeSyoString(syo_default_config_path());
    const QString databasePath = takeSyoString(syo_default_db_path());
    const bool configExists = !configPath.isEmpty() && QFileInfo::exists(configPath);
    line(QStringLiteral("  Config path:      %1").arg(configPath));
    line(QStringLiteral("  Config file:      %1")
             .arg(configExists ? QStringLiteral("loaded")
                               : QStringLiteral("not found — using built-in defaults")));
    line(QStringLiteral("  Database path:    %1").arg(databasePath));
    SyoApp *app = syo_app_new(configPath.toUtf8().constData(), nullptr);
    const QString warnings = app ? takeSyoString(syo_app_startup_warnings(app)) : QString();
    const QString openDir = app ? takeSyoString(syo_app_open_dir(app)) : QString();
    const QString openDirSrc = app ? takeSyoString(syo_app_open_dir_source(app)) : QString();
    if (app)
        syo_app_free(app);
    line(QStringLiteral("  Open dialog dir:  %1  (%2)")
             .arg(openDir.isEmpty() ? QStringLiteral("(none)") : openDir, openDirSrc));
    if (warnings.isEmpty())
        line(QStringLiteral("  Warnings:         none"));
    else {
        const QStringList lines = warnings.split(QLatin1Char('\n'), Qt::SkipEmptyParts);
        line(QStringLiteral("  Warnings:         %1").arg(lines.value(0)));
        for (int i = 1; i < lines.size(); ++i)
            line(QStringLiteral("                    %1").arg(lines.at(i)));
    }
    line({});

    line(QStringLiteral("Versions"));
    line(QStringLiteral("  syodep (shell):   %1").arg(QCoreApplication::applicationVersion()));
    line(QStringLiteral("  syodep (core):    %1").arg(takeSyoString(syo_core_version())));
    line(QStringLiteral("  Qt:               %1 (built) / %2 (runtime)")
             .arg(QStringLiteral(QT_VERSION_STR), QString::fromUtf8(qVersion())));
    line(QStringLiteral("  Build type:       %1").arg(QStringLiteral(SYODEP_BUILD_TYPE)));
    return out;
}

QString buildVersionReport(const PlatformInfo &info)
{
    QString out;
    out += QStringLiteral("syodep %1\n").arg(QCoreApplication::applicationVersion());
    out += QStringLiteral("  core:      %1\n").arg(takeSyoString(syo_core_version()));
    out += QStringLiteral("  Qt:        %1\n").arg(QString::fromUtf8(qVersion()));
    out += QStringLiteral("  platform:  %1\n").arg(info.osName.toLower());
    out += QStringLiteral("  build type: %1\n").arg(QStringLiteral(SYODEP_BUILD_TYPE));
    return out;
}

} // namespace syodep::diag
