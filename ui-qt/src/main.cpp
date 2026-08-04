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
#include <cstdlib>

#include <QAction>
#include <QDockWidget>
#include <QListView>

#include "core_controller.h"
#include "diagnostics.h"
#include "main_window.h"
#include "sidebar/annotation_sidebar.h"
#include "sidebar/annotations_panel.h"
#include "sidebar/highlight_list_model.h"
#include "syodep_ffi.h"

namespace {

void smokeStep(const char *msg)
{
    std::fprintf(stderr, "SMOKE: %s\n", msg);
    std::fflush(stderr);
    if (FILE *f = std::fopen("smoke-progress.txt", "a")) {
        std::fprintf(f, "%s\n", msg);
        std::fclose(f);
    }
}

[[noreturn]] void smokeFail(const QString &msg)
{
    const QByteArray utf8 = msg.toUtf8();
    std::fprintf(stderr, "SMOKE FAIL: %s\n", utf8.constData());
    std::fflush(stderr);
    if (FILE *f = std::fopen("smoke-progress.txt", "a")) {
        std::fprintf(f, "FAIL: %s\n", utf8.constData());
        std::fclose(f);
    }
    std::exit(1);
}

int runSmokeTest(const QString &pdfPath)
{
    std::remove("smoke-progress.txt");
    smokeStep("start");

    // Drive the core through CoreController without persistence so CI runs
    // do not touch the user database.
    syodep::CoreController core(syodep::CorePersistence::Disabled);
    if (!core.isValid())
        smokeFail(QStringLiteral("core construction"));
    core.setViewportSize(800.0f, 600.0f);
    if (!core.openDocument(pdfPath))
        smokeFail(QStringLiteral("cannot open %1").arg(pdfPath));
    const QVector<syodep::CoreVisiblePage> pages = core.visiblePages();
    if (pages.isEmpty())
        smokeFail(QStringLiteral("no visible pages"));
    const QImage image = core.renderPage(pages.first().page);
    if (image.isNull() || image.width() == 0 || image.height() == 0)
        smokeFail(QStringLiteral("render"));
    smokeStep("core render ok");

    // Annotation snapshot must be queryable (empty is fine for a fresh PDF).
    const syodep::HighlightSnapshot snapshot = core.highlightSnapshot();
    (void)snapshot;
    (void)core.statusText();
    (void)core.focusOverlay();
    (void)core.selectionOverlay();
    (void)core.highlightOverlay();

    // The annotation paths need a real database. Use a temp DB so CI never
    // writes the user profile store.
    QTemporaryDir tmp;
    if (!tmp.isValid())
        smokeFail(QStringLiteral("temp dir"));
    const QString dbPath = tmp.filePath(QStringLiteral("smoke.sqlite3"));
    syodep::CoreController annotationCore(dbPath);
    if (!annotationCore.isValid())
        smokeFail(QStringLiteral("annotation core construction"));
    annotationCore.setViewportSize(800.0f, 600.0f);
    if (!annotationCore.openDocument(pdfPath))
        smokeFail(QStringLiteral("annotation core open"));
    // Commit two highlights via the same keys the UI would send: the second
    // word first, so the list can only be right if it is sorted by position.
    // `f` is also a prefix of `fw`/…, so `f` then `w` resolves as `fw`
    // (focus_enter_word); that still yields two document-ordered highlights.
    for (const char *key : {"f", "w", "w", "a", "a", "b", "a", "a"})
        annotationCore.sendKey(QString::fromUtf8(key));
    const int coreHighlightCount =
        annotationCore.highlightSnapshot().items.size();
    smokeStep(qPrintable(
        QStringLiteral("highlights committed (core=%1)").arg(coreHighlightCount)));
    if (coreHighlightCount != 2)
        smokeFail(QStringLiteral("expected 2 core highlights, got %1")
                      .arg(coreHighlightCount));

    syodep::AnnotationSidebar sidebar(&annotationCore);
    sidebar.refreshAnnotations(true);
    if (sidebar.contentState() != syodep::AnnotationSidebar::ContentState::HighlightList)
        smokeFail(QStringLiteral("expected highlight list after commit"));
    if (sidebar.model()->rowCount() != 2) {
        smokeFail(QStringLiteral("expected 2 highlights, got %1")
                      .arg(sidebar.model()->rowCount()));
    }
    const auto *first = sidebar.model()->itemAt(0);
    const auto *second = sidebar.model()->itemAt(1);
    if (!first || !second)
        smokeFail(QStringLiteral("missing highlight item"));
    if (first->id <= second->id)
        smokeFail(QStringLiteral("expected document order, not id order"));

    // The list and the export must agree on order, or "the next highlight"
    // means two different things in two places.
    const QString expectedExport =
        QStringLiteral("# Highlights\n\n")
        + annotationCore.highlightMarkdown(first->id) + QStringLiteral("\n\n")
        + annotationCore.highlightMarkdown(second->id);
    const QString allMarkdown = annotationCore.allHighlightsMarkdown();
    if (allMarkdown != expectedExport)
        smokeFail(QStringLiteral("export order disagrees with the list"));

    const QString exportPath = tmp.filePath(QStringLiteral("highlights.md"));
    QString exportError;
    if (!syodep::writeHighlightsMarkdown(exportPath, allMarkdown, &exportError))
        smokeFail(QStringLiteral("export write: %1").arg(exportError));
    QFile exported(exportPath);
    if (!exported.open(QIODevice::ReadOnly)
        || QString::fromUtf8(exported.readAll()) != allMarkdown + QStringLiteral("\n")) {
        smokeFail(QStringLiteral("exported file content (wanted LF newlines)"));
    }
    exported.close();
    smokeStep("highlights export ok");

    // Deleting a Pending highlight: the row goes, the other stays, and the
    // selection lands on what took its place.
    sidebar.listView()->setCurrentIndex(sidebar.model()->index(0, 0));
    const qint64 survivor = second->id;
    if (!annotationCore.deleteHighlight(first->id))
        smokeFail(QStringLiteral("deleteHighlight"));
    sidebar.refreshAnnotations(true);
    if (sidebar.model()->rowCount() != 1
        || !sidebar.model()->itemAt(0)
        || sidebar.model()->itemAt(0)->id != survivor) {
        smokeFail(QStringLiteral("wrong highlight survived the delete"));
    }
    smokeStep("highlight delete ok");

    // Markdown annotation create → save → export → delete, via the same core
    // the Highlights path used (temp DB; never the user profile store).
    if (!annotationCore.hasPersistence())
        smokeFail(QStringLiteral("expected annotation persistence"));
    annotationCore.sendKey(QStringLiteral("f"));
    annotationCore.sendKey(QStringLiteral("w"));
    annotationCore.sendKey(QStringLiteral("n"));
    if (annotationCore.pendingAnnotationText().isEmpty())
        smokeFail(QStringLiteral("n did not capture a pending annotation"));
    qint64 annotationId = 0;
    if (!annotationCore.createTextAnnotation(QStringLiteral("smoke note"), &annotationId)
        || annotationId == 0) {
        smokeFail(QStringLiteral("createTextAnnotation"));
    }
    const QString annotationsMd = annotationCore.allTextAnnotationsMarkdown();
    if (!annotationsMd.startsWith(QStringLiteral("# Annotations\n\n"))
        || !annotationsMd.contains(QStringLiteral("smoke note"))) {
        smokeFail(QStringLiteral("annotations markdown format"));
    }
    const QString annotationsPath = tmp.filePath(QStringLiteral("annotations.md"));
    if (!syodep::writeTextAnnotationsMarkdown(annotationsPath, annotationsMd, &exportError)) {
        smokeFail(QStringLiteral("annotations export write: %1").arg(exportError));
    }
    QFile annotationsFile(annotationsPath);
    if (!annotationsFile.open(QIODevice::ReadOnly)
        || QString::fromUtf8(annotationsFile.readAll())
            != annotationsMd + QStringLiteral("\n")) {
        smokeFail(QStringLiteral("annotations exported file content (wanted LF newlines)"));
    }
    annotationsFile.close();
    if (!annotationCore.deleteTextAnnotation(annotationId))
        smokeFail(QStringLiteral("deleteTextAnnotation"));
    if (!annotationCore.allTextAnnotationsMarkdown().isEmpty())
        smokeFail(QStringLiteral("annotation survived delete"));
    smokeStep("annotation api ok");

    // And once through the actual widgets: construct, show, paint one frame.
    smokeStep("mainwindow construct");
    syodep::MainWindow window;
    smokeStep("mainwindow constructed");
    if (!window.annotationSidebar() || !window.annotationsDock())
        smokeFail(QStringLiteral("annotation sidebar not constructed"));
    if (window.annotationsDock()->allowedAreas() != Qt::RightDockWidgetArea
        || window.annotationsDock()->features() != QDockWidget::DockWidgetClosable) {
        smokeFail(QStringLiteral("highlights dock is movable or floatable"));
    }
    if (window.annotationSidebar()->contentState()
        != syodep::AnnotationSidebar::ContentState::NoDocument) {
        smokeFail(QStringLiteral("expected no-document sidebar state"));
    }
    if (window.visibleSidebarPage().has_value())
        smokeFail(QStringLiteral("sidebar should start closed"));
    if (!window.openDocument(pdfPath))
        smokeFail(QStringLiteral("MainWindow open %1").arg(pdfPath));
    smokeStep("mainwindow open ok");
    if (window.visibleSidebarPage().has_value())
        smokeFail(QStringLiteral("sidebar should stay closed after open"));
    window.annotationSidebar()->refreshAnnotations(true);
    const auto state = window.annotationSidebar()->contentState();
    if (state != syodep::AnnotationSidebar::ContentState::EmptyHighlights
        && state != syodep::AnnotationSidebar::ContentState::HighlightList) {
        smokeFail(QStringLiteral("unexpected sidebar state after open"));
    }

    window.show();
    QApplication::processEvents();
    smokeStep("mainwindow shown");

    // Highlights ↔ Annotations page matrix: self-toggle hides; cross-toggle
    // substitutes; hide leaves the canvas focused.
    window.showSidebarPage(syodep::SidebarPage::Highlights);
    if (window.visibleSidebarPage() != syodep::SidebarPage::Highlights
        || !window.highlightsToggleAction()->isChecked()
        || window.annotationsToggleAction()->isChecked()) {
        smokeFail(QStringLiteral("Highlights page not visible"));
    }
    smokeStep("highlights page ok");
    window.toggleAnnotationsSidebar();
    smokeStep("annotations page toggled");
    if (window.visibleSidebarPage() != syodep::SidebarPage::Annotations
        || !window.annotationsToggleAction()->isChecked()
        || window.highlightsToggleAction()->isChecked()) {
        smokeFail(QStringLiteral("Leader n substitute did not show Annotations"));
    }
    window.toggleAnnotationsSidebar();
    if (window.visibleSidebarPage().has_value()
        || window.annotationsToggleAction()->isChecked()
        || window.highlightsToggleAction()->isChecked()) {
        smokeFail(QStringLiteral("Annotations self-toggle did not hide"));
    }
    window.toggleHighlightsSidebar();
    if (window.visibleSidebarPage() != syodep::SidebarPage::Highlights)
        smokeFail(QStringLiteral("toggle did not restore Highlights"));
    window.hideSidebar();
    if (window.visibleSidebarPage().has_value())
        smokeFail(QStringLiteral("hideSidebar left a visible page"));
    smokeStep("sidebar matrix ok");

    QTimer::singleShot(0, &window, &QWidget::close);
    QApplication::processEvents();
    smokeStep("closed");

    std::printf("SMOKE OK\n");
    std::fflush(stdout);
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
    if (window.startFullscreen())
        window.showFullScreen();
    else
        window.show();
    return app.exec();
}
