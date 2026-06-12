// syodep entry point.
//
// Usage:
//   syodep [file.pdf]            open a window (optionally with a document)
//   syodep --smoke-test file.pdf headless render check, exits 0 on success
//
// The smoke-test mode exists for CI: it exercises window construction, the
// FFI boundary, document opening and a first paint without needing a real
// display (run with QT_QPA_PLATFORM=offscreen).

#include <QApplication>
#include <QCommandLineParser>
#include <QTimer>

#include <cstdio>

#include "main_window.h"
#include "syodep_ffi.h"

namespace {

int runSmokeTest(const QString &pdfPath)
{
    // Drive the core exactly like the window does, but without persistence
    // so CI runs do not touch the user database.
    SyoApp *app = syo_app_new(nullptr, nullptr);
    if (!app) {
        std::fprintf(stderr, "SMOKE FAIL: core construction\n");
        return 1;
    }
    syo_app_set_viewport(app, 800.0f, 600.0f);
    if (!syo_app_open_document(app, pdfPath.toUtf8().constData())) {
        std::fprintf(stderr, "SMOKE FAIL: cannot open %s\n", qPrintable(pdfPath));
        syo_app_free(app);
        return 1;
    }
    SyoVisiblePage pages[8];
    const size_t visible = syo_app_visible_pages(app, pages, 8);
    if (visible == 0) {
        std::fprintf(stderr, "SMOKE FAIL: no visible pages\n");
        syo_app_free(app);
        return 1;
    }
    SyoBitmap *bitmap = syo_app_render_page(app, pages[0].page);
    if (!bitmap || bitmap->width == 0 || bitmap->height == 0) {
        std::fprintf(stderr, "SMOKE FAIL: render\n");
        syo_bitmap_free(bitmap);
        syo_app_free(app);
        return 1;
    }
    syo_bitmap_free(bitmap);
    syo_app_free(app);

    // And once through the actual widgets: construct, show, paint one frame.
    syodep::MainWindow window;
    window.show();
    QTimer::singleShot(0, &window, &QWidget::close);
    QApplication::processEvents();

    std::printf("SMOKE OK\n");
    return 0;
}

} // namespace

int main(int argc, char *argv[])
{
    QApplication app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("syodep"));
    QApplication::setApplicationVersion(QStringLiteral("0.1.1"));

    QCommandLineParser parser;
    parser.setApplicationDescription(
        QStringLiteral("keyboard-first academic PDF reader"));
    parser.addHelpOption();
    parser.addVersionOption();
    parser.addPositionalArgument(QStringLiteral("file"),
                                 QStringLiteral("PDF document to open"));
    QCommandLineOption smokeOption(QStringLiteral("smoke-test"),
                                   QStringLiteral("render one frame and exit"));
    parser.addOption(smokeOption);
    parser.process(app);

    const QStringList args = parser.positionalArguments();

    if (parser.isSet(smokeOption)) {
        if (args.isEmpty()) {
            std::fprintf(stderr, "SMOKE FAIL: --smoke-test requires a PDF path\n");
            return 1;
        }
        return runSmokeTest(args.first());
    }

    syodep::MainWindow window;
    if (!args.isEmpty())
        window.openDocument(args.first());
    window.show();
    return app.exec();
}
