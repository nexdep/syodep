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

    // The annotation paths need a real database. Use a temp DB so CI never
    // writes the user profile store.
    QTemporaryDir tmp;
    if (!tmp.isValid()) {
        std::fprintf(stderr, "SMOKE FAIL: temp dir\n");
        return 1;
    }
    const QString dbPath = tmp.filePath(QStringLiteral("smoke.sqlite3"));
    syodep::CoreController annotationCore(dbPath);
    if (!annotationCore.isValid()) {
        std::fprintf(stderr, "SMOKE FAIL: annotation core construction\n");
        return 1;
    }
    annotationCore.setViewportSize(800.0f, 600.0f);
    if (!annotationCore.openDocument(pdfPath)) {
        std::fprintf(stderr, "SMOKE FAIL: annotation core open\n");
        return 1;
    }
    // Commit two highlights via the same keys the UI would send: the second
    // word first, so the list can only be right if it is sorted by position.
    for (const char *key : {"f", "w", "w", "a", "a", "b", "a", "a"})
        annotationCore.sendKey(QString::fromUtf8(key));

    syodep::AnnotationSidebar sidebar(&annotationCore);
    sidebar.refreshAnnotations(true);
    if (sidebar.contentState() != syodep::AnnotationSidebar::ContentState::HighlightList) {
        std::fprintf(stderr, "SMOKE FAIL: expected highlight list after commit\n");
        return 1;
    }
    if (sidebar.model()->rowCount() != 2) {
        std::fprintf(stderr, "SMOKE FAIL: expected 2 highlights, got %d\n",
                     sidebar.model()->rowCount());
        return 1;
    }
    const auto *first = sidebar.model()->itemAt(0);
    const auto *second = sidebar.model()->itemAt(1);
    if (!first || !second) {
        std::fprintf(stderr, "SMOKE FAIL: missing highlight item\n");
        return 1;
    }
    if (first->id <= second->id) {
        std::fprintf(stderr, "SMOKE FAIL: expected document order, not id order\n");
        return 1;
    }

    // The list and the export must agree on order, or "the next highlight"
    // means two different things in two places.
    const QString expectedExport =
        QStringLiteral("# Highlights\n\n")
        + annotationCore.highlightMarkdown(first->id) + QStringLiteral("\n\n")
        + annotationCore.highlightMarkdown(second->id);
    const QString allMarkdown = annotationCore.allHighlightsMarkdown();
    if (allMarkdown != expectedExport) {
        std::fprintf(stderr, "SMOKE FAIL: export order disagrees with the list\n");
        return 1;
    }

    const QString exportPath = tmp.filePath(QStringLiteral("highlights.md"));
    QString exportError;
    if (!syodep::writeHighlightsMarkdown(exportPath, allMarkdown, &exportError)) {
        std::fprintf(stderr, "SMOKE FAIL: export write: %s\n", qPrintable(exportError));
        return 1;
    }
    QFile exported(exportPath);
    if (!exported.open(QIODevice::ReadOnly)
        || QString::fromUtf8(exported.readAll()) != allMarkdown + QStringLiteral("\n")) {
        std::fprintf(stderr, "SMOKE FAIL: exported file content\n");
        return 1;
    }
    exported.close();

    // Deleting a Pending highlight: the row goes, the other stays, and the
    // selection lands on what took its place.
    sidebar.listView()->setCurrentIndex(sidebar.model()->index(0, 0));
    const qint64 survivor = second->id;
    if (!annotationCore.deleteHighlight(first->id)) {
        std::fprintf(stderr, "SMOKE FAIL: deleteHighlight\n");
        return 1;
    }
    sidebar.refreshAnnotations(true);
    if (sidebar.model()->rowCount() != 1
        || !sidebar.model()->itemAt(0)
        || sidebar.model()->itemAt(0)->id != survivor) {
        std::fprintf(stderr, "SMOKE FAIL: wrong highlight survived the delete\n");
        return 1;
    }

    // Markdown annotation create → save → export → delete, via the same core
    // the Highlights path used (temp DB; never the user profile store).
    if (!annotationCore.hasPersistence()) {
        std::fprintf(stderr, "SMOKE FAIL: expected annotation persistence\n");
        return 1;
    }
    annotationCore.sendKey(QStringLiteral("f"));
    annotationCore.sendKey(QStringLiteral("w"));
    annotationCore.sendKey(QStringLiteral("n"));
    if (annotationCore.pendingAnnotationText().isEmpty()) {
        std::fprintf(stderr, "SMOKE FAIL: n did not capture a pending annotation\n");
        return 1;
    }
    qint64 annotationId = 0;
    if (!annotationCore.createTextAnnotation(QStringLiteral("smoke note"), &annotationId)
        || annotationId == 0) {
        std::fprintf(stderr, "SMOKE FAIL: createTextAnnotation\n");
        return 1;
    }
    const QString annotationsMd = annotationCore.allTextAnnotationsMarkdown();
    if (!annotationsMd.startsWith(QStringLiteral("# Annotations\n\n"))
        || !annotationsMd.contains(QStringLiteral("smoke note"))) {
        std::fprintf(stderr, "SMOKE FAIL: annotations markdown format\n");
        return 1;
    }
    const QString annotationsPath = tmp.filePath(QStringLiteral("annotations.md"));
    if (!syodep::writeTextAnnotationsMarkdown(annotationsPath, annotationsMd, &exportError)) {
        std::fprintf(stderr, "SMOKE FAIL: annotations export write: %s\n",
                     qPrintable(exportError));
        return 1;
    }
    QFile annotationsFile(annotationsPath);
    if (!annotationsFile.open(QIODevice::ReadOnly)
        || QString::fromUtf8(annotationsFile.readAll())
            != annotationsMd + QStringLiteral("\n")) {
        std::fprintf(stderr, "SMOKE FAIL: annotations exported file content\n");
        return 1;
    }
    annotationsFile.close();
    if (!annotationCore.deleteTextAnnotation(annotationId)) {
        std::fprintf(stderr, "SMOKE FAIL: deleteTextAnnotation\n");
        return 1;
    }
    if (!annotationCore.allTextAnnotationsMarkdown().isEmpty()) {
        std::fprintf(stderr, "SMOKE FAIL: annotation survived delete\n");
        return 1;
    }

    // And once through the actual widgets: construct, show, paint one frame.
    syodep::MainWindow window;
    if (!window.annotationSidebar() || !window.annotationsDock()) {
        std::fprintf(stderr, "SMOKE FAIL: annotation sidebar not constructed\n");
        return 1;
    }
    if (window.annotationsDock()->allowedAreas() != Qt::RightDockWidgetArea
        || window.annotationsDock()->features() != QDockWidget::DockWidgetClosable) {
        std::fprintf(stderr, "SMOKE FAIL: highlights dock is movable or floatable\n");
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
    QApplication::processEvents();

    // Highlights ↔ Annotations page matrix: self-toggle hides; cross-toggle
    // substitutes; hide leaves the canvas focused.
    window.showSidebarPage(syodep::SidebarPage::Highlights);
    if (window.visibleSidebarPage() != syodep::SidebarPage::Highlights
        || !window.highlightsToggleAction()->isChecked()
        || window.annotationsToggleAction()->isChecked()) {
        std::fprintf(stderr, "SMOKE FAIL: Highlights page not visible\n");
        return 1;
    }
    window.toggleAnnotationsSidebar();
    if (window.visibleSidebarPage() != syodep::SidebarPage::Annotations
        || !window.annotationsToggleAction()->isChecked()
        || window.highlightsToggleAction()->isChecked()) {
        std::fprintf(stderr, "SMOKE FAIL: Leader n substitute did not show Annotations\n");
        return 1;
    }
    window.toggleAnnotationsSidebar();
    if (window.visibleSidebarPage().has_value()
        || window.annotationsToggleAction()->isChecked()
        || window.highlightsToggleAction()->isChecked()) {
        std::fprintf(stderr, "SMOKE FAIL: Annotations self-toggle did not hide\n");
        return 1;
    }
    window.toggleHighlightsSidebar();
    if (window.visibleSidebarPage() != syodep::SidebarPage::Highlights) {
        std::fprintf(stderr, "SMOKE FAIL: toggle did not restore Highlights\n");
        return 1;
    }
    window.hideSidebar();
    if (window.visibleSidebarPage().has_value()) {
        std::fprintf(stderr, "SMOKE FAIL: hideSidebar left a visible page\n");
        return 1;
    }

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
