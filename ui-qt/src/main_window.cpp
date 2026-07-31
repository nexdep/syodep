#include "main_window.h"

#include <QCloseEvent>
#include <QDockWidget>
#include <QDragEnterEvent>
#include <QDropEvent>
#include <QFileDialog>
#include <QFileInfo>
#include <QMenu>
#include <QMenuBar>
#include <QMessageBox>
#include <QMimeData>
#include <QPushButton>
#include <QStatusBar>
#include <QUrl>

#include "canvas_widget.h"
#include "core_controller.h"
#include "sidebar/annotation_sidebar.h"

namespace syodep {

namespace {

// Local .pdf paths carried by a drag, in the order they were dragged.
//
// Shared by dragEnterEvent and dropEvent so the two cannot disagree about
// what is acceptable -- otherwise a drag could show the "copy" cursor and
// then do nothing on release. toLocalFile() yields an empty string for
// remote URLs (http:, ftp:), which filters them out here.
QStringList droppablePdfs(const QMimeData *mime)
{
    QStringList paths;
    if (!mime || !mime->hasUrls())
        return paths;
    for (const QUrl &url : mime->urls()) {
        const QString path = url.toLocalFile();
        if (!path.isEmpty() && path.endsWith(QStringLiteral(".pdf"), Qt::CaseInsensitive))
            paths.append(path);
    }
    return paths;
}

} // namespace

MainWindow::MainWindow(QWidget *parent)
    : QMainWindow(parent)
{
    setWindowTitle(QStringLiteral("syodep"));
    resize(960, 1000);

    // Controller first so its lifetime covers the canvas.
    m_core = new CoreController(this);
    m_canvas = new CanvasWidget(m_core, this);
    // Overlay colours come from [view] in the config, resolved by the core.
    m_canvas->setBackgroundColor(m_core->backgroundColor());
    m_canvas->setFocusColor(m_core->focusColor());
    m_canvas->setVisualColor(m_core->visualColor());
    m_canvas->setHighlightColor(m_core->highlightColor());
    setCentralWidget(m_canvas);

    m_annotationsDock = new QDockWidget(tr("Highlights"), this);
    m_annotationsDock->setObjectName(QStringLiteral("highlightsDock"));
    m_annotationsDock->setAllowedAreas(Qt::LeftDockWidgetArea | Qt::RightDockWidgetArea);
    m_annotationSidebar = new AnnotationSidebar(m_core, m_annotationsDock);
    m_annotationsDock->setWidget(m_annotationSidebar);
    addDockWidget(Qt::RightDockWidgetArea, m_annotationsDock);
    // Reasonable initial width; user can still resize freely.
    resizeDocks({m_annotationsDock}, {340}, Qt::Horizontal);

    QMenu *viewMenu = menuBar()->addMenu(tr("&View"));
    viewMenu->addAction(m_annotationsDock->toggleViewAction());

    // The canvas covers the window but leaves acceptDrops() false, so Qt walks
    // up to the window for drag events. Only the window needs the flag.
    setAcceptDrops(true);

    m_status = new QLabel(this);
    m_status->setTextFormat(Qt::PlainText);
    statusBar()->addWidget(m_status, 1);

    connect(m_core, &CoreController::statusChanged, this, &MainWindow::refreshStatus);
    connect(m_core, &CoreController::quitRequested, this, &MainWindow::close);
    connect(m_core, &CoreController::openFileRequested, this, &MainWindow::showOpenDialog);
    connect(m_core, &CoreController::confirmQuitRequested,
            this, &MainWindow::onConfirmQuitRequested);

    const QString warnings = m_core->startupWarnings();
    if (!warnings.isEmpty())
        statusBar()->showMessage(warnings.section(QLatin1Char('\n'), 0, 0), 10000);

    refreshStatus();
}

bool MainWindow::openDocument(const QString &path)
{
    // Resolve unsaved comment drafts before the core replaces the document.
    if (m_annotationSidebar && !m_annotationSidebar->prepareForDocumentChange())
        return false;
    // Cache invalidation, redraw, and status refresh are emitted by the
    // controller — do not duplicate them here.
    return m_core->openDocument(path);
}

void MainWindow::dragEnterEvent(QDragEnterEvent *event)
{
    // Accepting only here is what makes the cursor show "no entry" for
    // anything else -- the user finds out before releasing.
    if (!droppablePdfs(event->mimeData()).isEmpty())
        event->acceptProposedAction();
}

void MainWindow::dropEvent(QDropEvent *event)
{
    const QStringList paths = droppablePdfs(event->mimeData());
    if (paths.isEmpty())
        return;
    event->acceptProposedAction();

    // One document at a time: open the first and say so, rather than
    // discarding the drop or silently ignoring the rest.
    openDocument(paths.first());
    if (paths.size() > 1) {
        // Spelled out rather than via tr()'s %n plural form: with no
        // translation catalogue loaded, tr() returns the source string
        // unchanged, so "file(s)" would reach the user literally.
        const int ignored = paths.size() - 1;
        const QString name = QFileInfo(paths.first()).fileName();
        const QString message = ignored == 1
            ? tr("Opened %1 - 1 other file ignored").arg(name)
            : tr("Opened %1 - %2 other files ignored").arg(name).arg(ignored);
        statusBar()->showMessage(message, 5000);
    }
}

void MainWindow::refreshStatus()
{
    m_status->setText(m_core->statusText());
}

bool MainWindow::confirmQuit()
{
    if (m_closeConfirmed)
        return true;

    // 1. Resolve unsaved comment drafts before Pending-highlight confirmation.
    if (m_annotationSidebar && !m_annotationSidebar->prepareForClose())
        return false;

    if (!m_core->hasUnsavedHighlights()) {
        // Nothing to discard, but this still saves the reading position
        // eagerly rather than relying on an implicit save elsewhere.
        m_core->quitDiscarding();
        m_closeConfirmed = true;
        return true;
    }

    QMessageBox box(this);
    box.setIcon(QMessageBox::Warning);
    box.setWindowTitle(tr("Quit"));
    box.setText(tr("This document has highlights that have not been saved "
                    "into the PDF yet."));
    QPushButton *saveBtn = box.addButton(tr("Save && Quit"), QMessageBox::AcceptRole);
    QPushButton *discardBtn = box.addButton(tr("Discard && Quit"), QMessageBox::DestructiveRole);
    box.addButton(QMessageBox::Cancel);
    box.setDefaultButton(saveBtn);
    box.exec();

    bool quit = false;
    if (box.clickedButton() == saveBtn)
        quit = m_core->quitSaving();
    else if (box.clickedButton() == discardBtn)
        quit = m_core->quitDiscarding();
    else
        return false; // Cancel

    if (!quit) {
        // Save failed: surface it. ReturnOnly avoided a re-entrant
        // quitRequested while closeEvent is deciding.
        m_canvas->update();
        refreshStatus();
        return false;
    }
    m_closeConfirmed = true;
    return true;
}

void MainWindow::closeEvent(QCloseEvent *event)
{
    if (confirmQuit())
        event->accept();
    else
        event->ignore();
}

void MainWindow::onConfirmQuitRequested()
{
    if (confirmQuit())
        close(); // re-enters closeEvent, short-circuited by m_closeConfirmed
}

void MainWindow::showOpenDialog()
{
    const QString start = m_core->openDirectory();
    const QString path = QFileDialog::getOpenFileName(
        this, tr("Open PDF"), start, tr("PDF documents (*.pdf);;All files (*)"));
    if (!path.isEmpty())
        openDocument(path);
}

} // namespace syodep
