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
#include <QFile>
#include <QFileInfo>
#include <QTemporaryDir>
#include <QTimer>

#include <cstdio>

#include "core_controller.h"
#include "diagnostics.h"
#include "main_window.h"
#include "sidebar/annotation_sidebar.h"
#include "sidebar/highlight_comment_editor.h"
#include "sidebar/highlight_list_model.h"
#include "syodep_ffi.h"

namespace {

int runSmokeTest(const QString &pdfPath)
{
    // Drive the core through CoreController without persistence so CI runs
    // do not touch the user database.
    syodep::CoreController core(syodep::CorePersistence::Disabled);
    if (!core.isValid()) {
        std::fprintf(stderr, "SMOKE FAIL: core construction\n");
        return 1;
    }
    core.setViewportSize(800.0f, 600.0f);
    if (!core.openDocument(pdfPath)) {
        std::fprintf(stderr, "SMOKE FAIL: cannot open %s\n", qPrintable(pdfPath));
        return 1;
    }
    const QVector<syodep::CoreVisiblePage> pages = core.visiblePages();
    if (pages.isEmpty()) {
        std::fprintf(stderr, "SMOKE FAIL: no visible pages\n");
        return 1;
    }
    const QImage image = core.renderPage(pages.first().page);
    if (image.isNull() || image.width() == 0 || image.height() == 0) {
        std::fprintf(stderr, "SMOKE FAIL: render\n");
        return 1;
    }

    // Annotation snapshot must be queryable (empty is fine for a fresh PDF).
    const syodep::HighlightSnapshot snapshot = core.highlightSnapshot();
    (void)snapshot;
    (void)core.statusText();
    (void)core.focusOverlay();
    (void)core.selectionOverlay();
    (void)core.highlightOverlay();

    // Comment path needs a real database. Use a temp DB so CI never writes the
    // user profile store.
    QTemporaryDir tmp;
    if (!tmp.isValid()) {
        std::fprintf(stderr, "SMOKE FAIL: temp dir\n");
        return 1;
    }
    const QString dbPath = tmp.filePath(QStringLiteral("smoke.sqlite3"));
    syodep::CoreController noteCore(dbPath);
    if (!noteCore.isValid()) {
        std::fprintf(stderr, "SMOKE FAIL: note core construction\n");
        return 1;
    }
    noteCore.setViewportSize(800.0f, 600.0f);
    if (!noteCore.openDocument(pdfPath)) {
        std::fprintf(stderr, "SMOKE FAIL: note core open\n");
        return 1;
    }
    // Commit one highlight via the same keys the UI would send.
    for (const char *key : {"f", "w", "a", "a"})
        noteCore.sendKey(QString::fromUtf8(key));

    syodep::AnnotationSidebar sidebar(&noteCore);
    sidebar.refreshAnnotations(true);
    if (sidebar.contentState() != syodep::AnnotationSidebar::ContentState::HighlightList) {
        std::fprintf(stderr, "SMOKE FAIL: expected highlight list after commit\n");
        return 1;
    }
    if (!sidebar.commentEditor()) {
        std::fprintf(stderr, "SMOKE FAIL: comment editor missing\n");
        return 1;
    }
    const auto *item = sidebar.model()->itemAt(0);
    if (!item) {
        std::fprintf(stderr, "SMOKE FAIL: missing highlight item\n");
        return 1;
    }
    sidebar.commentEditor()->loadHighlight(*item);
    if (sidebar.commentEditor()->isDirty() || sidebar.commentEditor()->hasHighlight() == false) {
        std::fprintf(stderr, "SMOKE FAIL: editor load\n");
        return 1;
    }
    if (!sidebar.commentEditor()->markdown().isEmpty()) {
        std::fprintf(stderr, "SMOKE FAIL: expected empty note\n");
        return 1;
    }

    const quint64 revBefore = noteCore.annotationRevision();
    const QString note = QStringLiteral("Smoke comment");
    if (!noteCore.setHighlightNote(item->id, note)) {
        std::fprintf(stderr, "SMOKE FAIL: setHighlightNote\n");
        return 1;
    }
    if (noteCore.annotationRevision() <= revBefore) {
        std::fprintf(stderr, "SMOKE FAIL: revision did not advance\n");
        return 1;
    }
    sidebar.refreshAnnotations(true);
    const auto *updated = sidebar.model()->itemAt(0);
    if (!updated || !updated->hasNote || updated->noteMarkdown != note) {
        std::fprintf(stderr, "SMOKE FAIL: note missing after refresh\n");
        return 1;
    }
    const QString exported = noteCore.highlightMarkdown(item->id);
    if (!exported.contains(note)) {
        std::fprintf(stderr, "SMOKE FAIL: export missing note\n");
        return 1;
    }

    // And once through the actual widgets: construct, show, paint one frame.
    syodep::MainWindow window;
    if (!window.annotationSidebar() || !window.annotationsDock()) {
        std::fprintf(stderr, "SMOKE FAIL: annotation sidebar not constructed\n");
        return 1;
    }
    if (window.annotationSidebar()->contentState()
        != syodep::AnnotationSidebar::ContentState::NoDocument) {
        std::fprintf(stderr, "SMOKE FAIL: expected no-document sidebar state\n");
        return 1;
    }
    if (!window.openDocument(pdfPath)) {
        std::fprintf(stderr, "SMOKE FAIL: MainWindow open %s\n", qPrintable(pdfPath));
        return 1;
    }
    window.annotationSidebar()->refreshAnnotations(true);
    const auto state = window.annotationSidebar()->contentState();
    if (state != syodep::AnnotationSidebar::ContentState::EmptyHighlights
        && state != syodep::AnnotationSidebar::ContentState::HighlightList) {
        std::fprintf(stderr, "SMOKE FAIL: unexpected sidebar state after open\n");
        return 1;
    }

    window.show();
    QTimer::singleShot(0, &window, &QWidget::close);
    QApplication::processEvents();

    std::printf("SMOKE OK\n");
    return 0;
}

// Write a documented config template (every option at its default) to
// `syodep_defaults.config.toml` in the current working directory, overwriting
// any existing file. Returns a process exit code.
int writeDefaultsConfig()
{
    char *raw = syo_default_config_toml();
    const QByteArray toml = raw ? QByteArray(raw) : QByteArray();
    syo_string_free(raw);

    const QString path = QStringLiteral("syodep_defaults.config.toml");
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        std::fprintf(stderr, "syodep: cannot write %s: %s\n",
                     qPrintable(QFileInfo(path).absoluteFilePath()),
                     qPrintable(file.errorString()));
        return 1;
    }
    if (file.write(toml) != toml.size()) {
        std::fprintf(stderr, "syodep: failed writing %s: %s\n",
                     qPrintable(QFileInfo(path).absoluteFilePath()),
                     qPrintable(file.errorString()));
        return 1;
    }
    std::printf("Wrote %s\n", qPrintable(QFileInfo(path).absoluteFilePath()));
    return 0;
}

} // namespace

int main(int argc, char *argv[])
{
    // These are static and safe to set before the QApplication exists; the
    // version reporters below read them without needing a display.
    QApplication::setApplicationName(QStringLiteral("syodep"));
    QApplication::setApplicationVersion(QStringLiteral(SYODEP_VERSION));

    // Decide and apply graphics fallbacks (software GL, X11 on WSL, ...) before
    // the QApplication is constructed — Qt locks the platform plugin and GL
    // backend in at that point. Silent on a normal launch; `--check` reports it.
    const syodep::diag::PlatformInfo platform = syodep::diag::detectPlatform();
    const syodep::diag::GraphicsDecision decision =
        syodep::diag::decideFallbacks(platform);
    syodep::diag::applyFallbacks(decision);

    // --version and --defaults need neither a display nor a GL context, so
    // handle them before constructing QApplication (which would pull in the
    // platform plugin and fail on a headless machine).
    for (int i = 1; i < argc; ++i) {
        const QByteArray arg(argv[i]);
        if (arg == "--version" || arg == "-v") {
            std::fputs(qPrintable(syodep::diag::buildVersionReport(platform)), stdout);
            return 0;
        }
        if (arg == "--defaults")
            return writeDefaultsConfig();
    }

    QApplication app(argc, argv);

    QCommandLineParser parser;
    parser.setApplicationDescription(
        QStringLiteral("keyboard-first academic PDF reader"));
    parser.addHelpOption();
    // Listed for --help; the actual handling happens before QApplication above.
    QCommandLineOption versionOption({QStringLiteral("v"), QStringLiteral("version")},
                                     QStringLiteral("show version information and exit"));
    parser.addOption(versionOption);
    QCommandLineOption checkOption(QStringLiteral("check"),
                                   QStringLiteral("print graphics/config diagnostics and exit"));
    parser.addOption(checkOption);
    QCommandLineOption defaultsOption(
        QStringLiteral("defaults"),
        QStringLiteral("write a documented syodep_defaults.config.toml to the "
                       "current directory and exit"));
    parser.addOption(defaultsOption);
    parser.addPositionalArgument(QStringLiteral("file"),
                                 QStringLiteral("PDF document to open"));
    QCommandLineOption smokeOption(QStringLiteral("smoke-test"),
                                   QStringLiteral("render one frame and exit"));
    parser.addOption(smokeOption);
    parser.process(app);

    if (parser.isSet(checkOption)) {
        std::fputs(qPrintable(syodep::diag::buildCheckReport(platform, decision)), stdout);
        return 0;
    }

    // --defaults is handled in the early argv scan above (no display needed);
    // the option is registered only so it appears in --help.

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
