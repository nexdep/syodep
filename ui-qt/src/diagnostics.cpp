#include "diagnostics.h"

#include <QCoreApplication>
#include <QFile>
#include <QFileInfo>
#include <QGuiApplication>
#include <QOffscreenSurface>
#include <QOpenGLContext>
#include <QOpenGLFunctions>
#include <QtGlobal>

#include "syodep_ffi.h"

#ifndef SYODEP_BUILD_TYPE
#define SYODEP_BUILD_TYPE "unknown"
#endif

namespace syodep::diag {

namespace {

// Take ownership of a string returned by the core and free the FFI buffer.
QString takeSyoString(char *s)
{
    if (!s)
        return {};
    const QString out = QString::fromUtf8(s);
    syo_string_free(s);
    return out;
}

// Read /proc/version once; used to recognise a WSL kernel.
QString procVersion()
{
    QFile f(QStringLiteral("/proc/version"));
    if (!f.open(QIODevice::ReadOnly | QIODevice::Text))
        return {};
    return QString::fromUtf8(f.readAll());
}

} // namespace

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

    info.displayValue = qEnvironmentVariable("DISPLAY");
    info.hasDisplay = !info.displayValue.isEmpty();
    info.waylandValue = qEnvironmentVariable("WAYLAND_DISPLAY");
    info.hasWaylandDisplay = !info.waylandValue.isEmpty();

#if defined(Q_OS_LINUX)
    // WSL exposes WSL_DISTRO_NAME in every distro shell; the kernel string is
    // the fallback signal. GPU passthrough shows up as /dev/dxg.
    const QString distro = qEnvironmentVariable("WSL_DISTRO_NAME");
    if (!distro.isEmpty()) {
        info.isWsl = true;
        info.wslSignal = QStringLiteral("WSL_DISTRO_NAME=%1").arg(distro);
    } else {
        const QString ver = procVersion();
        if (ver.contains(QStringLiteral("microsoft"), Qt::CaseInsensitive)
            || ver.contains(QStringLiteral("WSL"), Qt::CaseInsensitive)) {
            info.isWsl = true;
            info.wslSignal = QStringLiteral("/proc/version mentions Microsoft/WSL");
        }
    }
    info.hasDxg = QFileInfo::exists(QStringLiteral("/dev/dxg"));
#endif

    return info;
}

GraphicsDecision decideFallbacks(const PlatformInfo &info)
{
    GraphicsDecision d;

    // If the user already pinned the platform or GL backend, respect it
    // entirely and apply nothing of our own.
    if (qEnvironmentVariableIsSet("QT_QPA_PLATFORM")
        || qEnvironmentVariableIsSet("LIBGL_ALWAYS_SOFTWARE")
        || qEnvironmentVariableIsSet("QT_OPENGL")) {
        d.userOverride = true;
        d.overrideReason = QStringLiteral("environment override present");
        return d;
    }

    // WSL is the one environment that needs help: the WSLg Wayland EGL path is
    // routinely broken, and without /dev/dxg there is no GPU to render on.
    if (info.isWsl) {
        if (info.hasDisplay) {
            d.forcePlatform = true;
            d.platform = QStringLiteral("xcb");
            d.platformReason = QStringLiteral("WSL: prefer X11 over broken WSLg wayland-egl");
        }
        if (!info.hasDxg) {
            d.forceSoftwareGl = true;
            d.softwareReason = QStringLiteral("WSL: no GPU passthrough (/dev/dxg missing)");
        }
    }

    return d;
}

void applyFallbacks(const GraphicsDecision &decision)
{
    if (decision.userOverride)
        return;

    if (decision.forcePlatform)
        qputenv("QT_QPA_PLATFORM", decision.platform.toUtf8());

    // AA_UseSoftwareOpenGL is the cross-platform "software" switch: Mesa
    // llvmpipe on Linux, opengl32sw on Windows. Must be set before QApplication.
    if (decision.forceSoftwareGl)
        QCoreApplication::setAttribute(Qt::AA_UseSoftwareOpenGL);
}

GlProbe probeOpenGl()
{
    GlProbe probe;

    QOpenGLContext ctx;
    if (!ctx.create()) {
        probe.error = QStringLiteral("QOpenGLContext::create() failed");
        return probe;
    }

    QOffscreenSurface surface;
    surface.setFormat(ctx.format());
    surface.create();
    if (!surface.isValid()) {
        probe.error = QStringLiteral("offscreen surface invalid");
        return probe;
    }
    if (!ctx.makeCurrent(&surface)) {
        probe.error = QStringLiteral("makeCurrent failed");
        return probe;
    }

    auto *f = ctx.functions();
    const auto str = [f](GLenum name) {
        const GLubyte *s = f->glGetString(name);
        return s ? QString::fromUtf8(reinterpret_cast<const char *>(s)) : QString();
    };
    probe.renderer = str(GL_RENDERER);
    probe.version = str(GL_VERSION);
    probe.vendor = str(GL_VENDOR);
    probe.ok = !probe.renderer.isEmpty();
    ctx.doneCurrent();
    return probe;
}

QString buildCheckReport(const PlatformInfo &info, const GraphicsDecision &decision)
{
    QString out;
    const auto line = [&out](const QString &s) { out += s + QLatin1Char('\n'); };

    line(QStringLiteral("syodep --check"));
    line({});

    // --- Platform -----------------------------------------------------------
    line(QStringLiteral("Platform"));
    QString os = info.osName;
    if (info.isWsl)
        os += QStringLiteral(" (WSL: %1)").arg(info.wslSignal);
    line(QStringLiteral("  OS:               %1").arg(os));
#if defined(Q_OS_LINUX)
    if (info.isWsl)
        line(QStringLiteral("  GPU passthrough:  %1")
                 .arg(info.hasDxg ? QStringLiteral("present (/dev/dxg)")
                                  : QStringLiteral("none (/dev/dxg missing)")));
    line(QStringLiteral("  Display:          DISPLAY=%1 ; WAYLAND_DISPLAY=%2")
             .arg(info.hasDisplay ? info.displayValue : QStringLiteral("unset"),
                  info.hasWaylandDisplay ? info.waylandValue : QStringLiteral("unset")));
#endif
    line({});

    // --- Graphics -----------------------------------------------------------
    line(QStringLiteral("Graphics"));
    QString platReason;
    if (decision.userOverride)
        platReason = QStringLiteral("user override");
    else if (decision.forcePlatform)
        platReason = QStringLiteral("forced: %1").arg(decision.platformReason);
    else
        platReason = QStringLiteral("auto");
    line(QStringLiteral("  Qt platform:      %1  (%2)")
             .arg(QGuiApplication::platformName(), platReason));

    QString glMode;
    if (decision.userOverride)
        glMode = QStringLiteral("per environment override");
    else if (decision.forceSoftwareGl)
        glMode = QStringLiteral("software (auto fallback: %1)").arg(decision.softwareReason);
    else
        glMode = QStringLiteral("hardware (default)");
    line(QStringLiteral("  OpenGL:           %1").arg(glMode));

    const GlProbe probe = probeOpenGl();
    if (probe.ok) {
        line(QStringLiteral("  GL renderer:      %1").arg(probe.renderer));
        line(QStringLiteral("  GL version:       %1").arg(probe.version));
        line(QStringLiteral("  Context:          OK"));
    } else {
        line(QStringLiteral("  Context:          FAILED (%1)").arg(probe.error));
    }
    line({});

    // --- Configuration ------------------------------------------------------
    line(QStringLiteral("Configuration"));
    const QString configPath = takeSyoString(syo_default_config_path());
    const bool configExists = !configPath.isEmpty() && QFileInfo::exists(configPath);
    line(QStringLiteral("  Config path:      %1").arg(configPath));
    line(QStringLiteral("  Config file:      %1")
             .arg(configExists ? QStringLiteral("loaded")
                               : QStringLiteral("not found — using built-in defaults")));
    // Construct a throwaway core (no persistence) to surface parse warnings.
    SyoApp *app = syo_app_new(configPath.toUtf8().constData(), nullptr);
    const QString warnings = app ? takeSyoString(syo_app_startup_warnings(app)) : QString();
    if (app)
        syo_app_free(app);
    if (warnings.isEmpty())
        line(QStringLiteral("  Warnings:         none"));
    else {
        const QStringList lines = warnings.split(QLatin1Char('\n'), Qt::SkipEmptyParts);
        line(QStringLiteral("  Warnings:         %1").arg(lines.value(0)));
        for (int i = 1; i < lines.size(); ++i)
            line(QStringLiteral("                    %1").arg(lines.at(i)));
    }
    line({});

    // --- Versions -----------------------------------------------------------
    line(QStringLiteral("Versions"));
    line(QStringLiteral("  syodep (shell):   %1").arg(QCoreApplication::applicationVersion()));
    line(QStringLiteral("  syodep (core):    %1").arg(takeSyoString(syo_core_version())));
    line(QStringLiteral("  Qt:               %1 (built) / %2 (runtime)")
             .arg(QStringLiteral(QT_VERSION_STR), QString::fromUtf8(qVersion())));
    line(QStringLiteral("  Build:            %1").arg(QStringLiteral(SYODEP_BUILD_TYPE)));

    return out;
}

QString buildVersionReport(const PlatformInfo &info)
{
    QString platform = info.osName.toLower();
    if (info.isWsl)
        platform += QStringLiteral(" (wsl)");

    QString out;
    out += QStringLiteral("syodep %1\n").arg(QCoreApplication::applicationVersion());
    out += QStringLiteral("  core:      %1\n").arg(takeSyoString(syo_core_version()));
    out += QStringLiteral("  Qt:        %1\n").arg(QString::fromUtf8(qVersion()));
    out += QStringLiteral("  platform:  %1\n").arg(platform);
    out += QStringLiteral("  build:     %1\n").arg(QStringLiteral(SYODEP_BUILD_TYPE));
    return out;
}

} // namespace syodep::diag
