// syodep entry point.
//
// Usage:
//   syodep [file.pdf]            open a window (optionally with a document)
//   syodep --smoke-test file.pdf Wayland render check, exits 0 on success
//
// The smoke-test mode exists for CI: it exercises window construction, the
// FFI boundary, document opening and a first paint under a real compositor.

#include <QApplication>
#include <QCommandLineParser>
#include <QFile>
#include <QFileInfo>
#include <QTemporaryDir>
#include <QTextCursor>
#include <QTimer>
#include <QWidget>

#include <cstdio>
#include <cstdlib>

#include <QAction>
#include <QDockWidget>
#include <QListView>
#include <QKeyEvent>
#include <QPlainTextEdit>

#include "canvas_widget.h"
#include "core_controller.h"
#include "diagnostics.h"
#include "main_window.h"
#include "key_encoder.h"
#include "keybindings_overlay.h"
#include "sidebar/annotation_sidebar.h"
#include "sidebar/annotations_panel.h"
#include "sidebar/highlight_list_model.h"
#include "sidebar/text_annotation_editor.h"
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

void showMainWindow(syodep::MainWindow &window)
{
    if (window.startFullscreen()) {
#if defined(Q_OS_LINUX)
        // Wayland compositors decide which newly mapped window receives focus.
        // Qt 6.2 otherwise issues an unsupported requestActivate() while
        // mapping a fullscreen window and prints a warning, with no change in
        // compositor behaviour.
        window.setAttribute(Qt::WA_ShowWithoutActivating);
#endif
        window.showFullScreen();
    } else {
        window.show();
    }
}

int runSmokeTest(const QString &pdfPath, syodep::RendererBackend renderer)
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

    // Regression for the real failure mode: once a populated sidebar list has
    // focus, leader sequences must still reach the isolated core context.
    int highlightsToggleRequests = 0;
    int annotationsToggleRequests = 0;
    int closeSidebarRequests = 0;
    QObject::connect(&annotationCore, &syodep::CoreController::toggleHighlightsSidebarRequested,
                     &sidebar, [&]() { ++highlightsToggleRequests; });
    QObject::connect(&annotationCore, &syodep::CoreController::toggleAnnotationsSidebarRequested,
                     &sidebar, [&]() { ++annotationsToggleRequests; });
    QObject::connect(&annotationCore, &syodep::CoreController::closeSidebarRequested,
                     &sidebar, [&]() {
                         ++closeSidebarRequests;
                         sidebar.hide();
                     });
    sidebar.resize(720, 500);
    {
        syodep::diag::FallbackStderrCapture startupStderr;
        sidebar.show();
        sidebar.focusList();
        QApplication::processEvents();
        startupStderr.finish(sidebar.isVisible());
    }
    QKeyEvent sidebarLeaderA1(QEvent::KeyPress, Qt::Key_Space,
                              Qt::NoModifier, QStringLiteral(" "));
    QKeyEvent sidebarLeaderA2(QEvent::KeyPress, Qt::Key_A,
                              Qt::NoModifier, QStringLiteral("a"));
    QApplication::sendEvent(sidebar.listView(), &sidebarLeaderA1);
    QApplication::sendEvent(sidebar.listView(), &sidebarLeaderA2);
    if (highlightsToggleRequests != 1)
        smokeFail(QStringLiteral("focused highlight list swallowed leader-a"));
    QKeyEvent sidebarLeaderN1(QEvent::KeyPress, Qt::Key_Space,
                              Qt::NoModifier, QStringLiteral(" "));
    QKeyEvent sidebarLeaderN2(QEvent::KeyPress, Qt::Key_N,
                              Qt::NoModifier, QStringLiteral("n"));
    QApplication::sendEvent(sidebar.listView(), &sidebarLeaderN1);
    QApplication::sendEvent(sidebar.listView(), &sidebarLeaderN2);
    if (annotationsToggleRequests != 1)
        smokeFail(QStringLiteral("focused highlight list swallowed leader-n"));
    QKeyEvent sidebarEscape(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
    QApplication::sendEvent(sidebar.listView(), &sidebarEscape);
    QApplication::processEvents();
    if (closeSidebarRequests != 1 || sidebar.isVisible())
        smokeFail(QStringLiteral("Escape did not close focused highlight sidebar"));
    smokeStep("focused sidebar input ok");

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

    // Editable Markdown deliberately keeps leader-shaped prose as text, while
    // Escape still closes the sidebar and preserves the dirty draft.
    annotationCore.sendKey(QStringLiteral("n"));
    qint64 draftId = 0;
    if (!annotationCore.createTextAnnotation(QStringLiteral("draft"), &draftId)
        || draftId == 0) {
        smokeFail(QStringLiteral("create draft annotation"));
    }
    sidebar.showPage(syodep::SidebarPage::Annotations);
    sidebar.show();
    QApplication::processEvents();
    auto *annotationsPanel = sidebar.annotationsPanel();
    if (!annotationsPanel || !annotationsPanel->editor())
        smokeFail(QStringLiteral("annotations editor construction"));
    annotationsPanel->focusList();
    QKeyEvent annotationLeaderA1(QEvent::KeyPress, Qt::Key_Space,
                                  Qt::NoModifier, QStringLiteral(" "));
    QKeyEvent annotationLeaderA2(QEvent::KeyPress, Qt::Key_A,
                                  Qt::NoModifier, QStringLiteral("a"));
    QApplication::sendEvent(annotationsPanel->listView(), &annotationLeaderA1);
    QApplication::sendEvent(annotationsPanel->listView(), &annotationLeaderA2);
    QKeyEvent annotationLeaderN1(QEvent::KeyPress, Qt::Key_Space,
                                  Qt::NoModifier, QStringLiteral(" "));
    QKeyEvent annotationLeaderN2(QEvent::KeyPress, Qt::Key_N,
                                  Qt::NoModifier, QStringLiteral("n"));
    QApplication::sendEvent(annotationsPanel->listView(), &annotationLeaderN1);
    QApplication::sendEvent(annotationsPanel->listView(), &annotationLeaderN2);
    if (highlightsToggleRequests != 2 || annotationsToggleRequests != 2)
        smokeFail(QStringLiteral("focused annotation list swallowed a leader toggle"));
    annotationsPanel->editor()->focusEditor();
    auto *markdownEditor = annotationsPanel->editor()->findChild<QPlainTextEdit *>();
    if (!markdownEditor)
        smokeFail(QStringLiteral("Markdown editor not found"));
    const QString beforeTyping = markdownEditor->toPlainText();
    markdownEditor->moveCursor(QTextCursor::End);
    const int togglesBeforeTyping =
        highlightsToggleRequests + annotationsToggleRequests;
    QKeyEvent typedSpace(QEvent::KeyPress, Qt::Key_Space,
                         Qt::NoModifier, QStringLiteral(" "));
    QKeyEvent typedA(QEvent::KeyPress, Qt::Key_A,
                     Qt::NoModifier, QStringLiteral("a"));
    QApplication::sendEvent(markdownEditor, &typedSpace);
    QApplication::sendEvent(markdownEditor, &typedA);
    if (markdownEditor->toPlainText() != beforeTyping + QStringLiteral(" a")
        || highlightsToggleRequests + annotationsToggleRequests != togglesBeforeTyping) {
        smokeFail(QStringLiteral("leader-shaped Markdown text was intercepted"));
    }
    const QString dirtyDraft = markdownEditor->toPlainText();
    QKeyEvent editorEscape(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
    QApplication::sendEvent(markdownEditor, &editorEscape);
    QApplication::processEvents();
    if (sidebar.isVisible() || closeSidebarRequests != 2)
        smokeFail(QStringLiteral("Escape did not close dirty annotation editor"));
    sidebar.show();
    sidebar.focusActivePage();
    QApplication::processEvents();
    if (markdownEditor->toPlainText() != dirtyDraft
        || !annotationsPanel->editor()->isDirty()) {
        smokeFail(QStringLiteral("dirty annotation draft did not survive sidebar close"));
    }
    sidebar.hide();
    smokeStep("annotation api ok");

    // And once through the actual widgets: construct, show, paint one frame.
    smokeStep("mainwindow construct");
    syodep::MainWindow window(renderer);
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
    if (window.startSidebarOpen())
        smokeFail(QStringLiteral("default start_sidebar_open should be false"));
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

    syodep::diag::FallbackStderrCapture windowStderr;
    showMainWindow(window);
    QApplication::processEvents();
    windowStderr.finish(window.isVisible());
    smokeStep("mainwindow shown");

    // The real Qt key encoder and modal overlay: Ctrl+Shift+? is represented
    // as the config syntax `<C-?>`; help opens over the window, scrolls with
    // its isolated keymap, then returns focus to the canvas on either close
    // path.
    QKeyEvent encodedQuestion(QEvent::KeyPress,
                              Qt::Key_Question,
                              Qt::ControlModifier | Qt::ShiftModifier,
                              QStringLiteral("?"));
    if (syodep::encodeKeyEvent(&encodedQuestion) != QStringLiteral("<C-?>"))
        smokeFail(QStringLiteral("Ctrl+? key encoding"));
    QWidget *canvas = window.canvasWidget();
    auto *help = window.keybindingsOverlay();
    if (!canvas || !help)
        smokeFail(QStringLiteral("keybinding overlay construction"));
    // A headless Weston session may leave QWidget focus unobservable even
    // though setFocus() is called correctly. Preserve the
    // focus-restoration assertion on platforms that could focus the canvas
    // before help opened; closure itself is mandatory everywhere.
    const bool canvasFocusIsObservable = canvas->hasFocus();
    QApplication::sendEvent(canvas, &encodedQuestion);
    QApplication::processEvents();
    if (!help->isVisible() || help->bindingCount() == 0)
        smokeFail(QStringLiteral("Ctrl+? did not show populated keybinding help"));
    QKeyEvent helpBottom(QEvent::KeyPress, Qt::Key_G, Qt::ShiftModifier, QStringLiteral("G"));
    QApplication::sendEvent(help, &helpBottom);
    QApplication::processEvents();
    if (help->scrollValue() != help->maximumScrollValue())
        smokeFail(QStringLiteral("G did not navigate help to the bottom"));
    QKeyEvent closeQuestion(QEvent::KeyPress,
                            Qt::Key_Question,
                            Qt::ControlModifier | Qt::ShiftModifier,
                            QStringLiteral("?"));
    QApplication::sendEvent(help, &closeQuestion);
    QApplication::processEvents();
    if (help->isVisible() || (canvasFocusIsObservable && !canvas->hasFocus()))
        smokeFail(QStringLiteral("Ctrl+? did not close help and restore canvas focus"));
    QApplication::sendEvent(canvas, &encodedQuestion);
    QApplication::processEvents();
    QKeyEvent escapeHelp(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
    QApplication::sendEvent(help, &escapeHelp);
    QApplication::processEvents();
    if (help->isVisible())
        smokeFail(QStringLiteral("Escape did not close keybinding help"));
    smokeStep("keybinding overlay ok");

    // Highlights ↔ Annotations page matrix through actual focused-sidebar key
    // events: self-toggle hides, cross-toggle substitutes, and Escape closes.
    auto sidebarKeyTarget = [&]() -> QWidget * {
        QWidget *target = QApplication::focusWidget();
        if (!target
            || (target != window.annotationSidebar()
                && !window.annotationSidebar()->isAncestorOf(target))) {
            target = window.annotationSidebar();
        }
        return target;
    };
    auto sendSidebarLeader = [&](int key, const QString &text) {
        QKeyEvent leader(QEvent::KeyPress, Qt::Key_Space,
                         Qt::NoModifier, QStringLiteral(" "));
        QApplication::sendEvent(sidebarKeyTarget(), &leader);
        QKeyEvent suffix(QEvent::KeyPress, key, Qt::NoModifier, text);
        QApplication::sendEvent(sidebarKeyTarget(), &suffix);
        QApplication::processEvents();
    };
    window.showSidebarPage(syodep::SidebarPage::Highlights);
    if (window.visibleSidebarPage() != syodep::SidebarPage::Highlights
        || !window.highlightsToggleAction()->isChecked()
        || window.annotationsToggleAction()->isChecked()) {
        smokeFail(QStringLiteral("Highlights page not visible"));
    }
    smokeStep("highlights page ok");
    sendSidebarLeader(Qt::Key_N, QStringLiteral("n"));
    smokeStep("annotations page toggled");
    if (window.visibleSidebarPage() != syodep::SidebarPage::Annotations
        || !window.annotationsToggleAction()->isChecked()
        || window.highlightsToggleAction()->isChecked()) {
        smokeFail(QStringLiteral("Leader n substitute did not show Annotations"));
    }
    sendSidebarLeader(Qt::Key_N, QStringLiteral("n"));
    if (window.visibleSidebarPage().has_value()
        || window.annotationsToggleAction()->isChecked()
        || window.highlightsToggleAction()->isChecked()) {
        smokeFail(QStringLiteral("Annotations self-toggle did not hide"));
    }
    window.showSidebarPage(syodep::SidebarPage::Highlights);
    if (window.visibleSidebarPage() != syodep::SidebarPage::Highlights)
        smokeFail(QStringLiteral("toggle did not restore Highlights"));
    sendSidebarLeader(Qt::Key_N, QStringLiteral("n"));
    if (window.visibleSidebarPage() != syodep::SidebarPage::Annotations)
        smokeFail(QStringLiteral("leader-n did not substitute Annotations"));
    sendSidebarLeader(Qt::Key_A, QStringLiteral("a"));
    if (window.visibleSidebarPage() != syodep::SidebarPage::Highlights)
        smokeFail(QStringLiteral("leader-a did not substitute Highlights"));
    sendSidebarLeader(Qt::Key_A, QStringLiteral("a"));
    if (window.visibleSidebarPage().has_value())
        smokeFail(QStringLiteral("leader-a self-toggle did not hide"));
    window.showSidebarPage(syodep::SidebarPage::Annotations);
    QKeyEvent closeSidebar(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
    QApplication::sendEvent(sidebarKeyTarget(), &closeSidebar);
    QApplication::processEvents();
    if (window.visibleSidebarPage().has_value())
        smokeFail(QStringLiteral("Escape left a visible sidebar page"));
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
    QApplication::setApplicationVersion(QStringLiteral(SYODEP_BUILD_VERSION));

    const syodep::diag::PlatformInfo platform = syodep::diag::detectPlatform();

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

    // Linux is intentionally Wayland-only. Select/reject the QPA before
    // QApplication locks it in; the headless commands above remain available
    // even when no compositor is running.
    QString platformError;
    if (!syodep::diag::configurePlatform(argc, argv, &platformError)) {
        std::fprintf(stderr, "syodep: %s\n", qPrintable(platformError));
        return 2;
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
    QCommandLineOption rendererOption(
        QStringLiteral("renderer"),
        QStringLiteral("select canvas renderer: auto, opengl, or raster"),
        QStringLiteral("mode"),
        QStringLiteral("auto"));
    parser.addOption(rendererOption);
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

    bool rendererOk = false;
    const syodep::diag::RendererPreference preference =
        syodep::diag::parseRendererPreference(parser.value(rendererOption), &rendererOk);
    if (!rendererOk) {
        std::fprintf(stderr,
                     "syodep: invalid renderer '%s' (expected auto, opengl, or raster)\n",
                     qPrintable(parser.value(rendererOption)));
        return 2;
    }

    syodep::diag::GlProbe glProbe;
    if (preference != syodep::diag::RendererPreference::Raster)
        glProbe = syodep::diag::probeOpenGlWidget();
    const syodep::diag::RendererDecision renderer =
        syodep::diag::decideRenderer(preference, glProbe);

    if (parser.isSet(checkOption)) {
        std::fputs(qPrintable(syodep::diag::buildCheckReport(platform, renderer, glProbe)),
                   stdout);
        return renderer.usable ? 0 : 2;
    }

    if (!renderer.usable) {
        std::fprintf(stderr,
                     "syodep: OpenGL renderer requested but unavailable: %s\n",
                     qPrintable(renderer.reason));
        return 2;
    }

    // --defaults is handled in the early argv scan above (no display needed);
    // the option is registered only so it appears in --help.

    const QStringList args = parser.positionalArguments();

    if (parser.isSet(smokeOption)) {
        if (args.isEmpty()) {
            std::fprintf(stderr, "SMOKE FAIL: --smoke-test requires a PDF path\n");
            return 1;
        }
        return runSmokeTest(args.first(), renderer.selected);
    }

    const QString rendererWarning = renderer.fellBack
        ? QStringLiteral("OpenGL unavailable; using raster renderer: %1").arg(renderer.reason)
        : QString();
    // The renderer probe is already bounded by its own capture. The actual
    // first window mapping is a separate operation: raster startup can make
    // Wayland-EGL try Zink before falling back to shared memory, and Qt 6.2
    // can request unsupported activation for a fullscreen mapping. Keep every
    // diagnostic if startup fails; otherwise discard only those known lines.
    syodep::diag::FallbackStderrCapture startupStderr;
    syodep::MainWindow window(renderer.selected, rendererWarning);
    if (!args.isEmpty())
        window.openDocument(args.first());
    showMainWindow(window);
    QApplication::processEvents();
    startupStderr.finish(window.isVisible());
    return app.exec();
}
