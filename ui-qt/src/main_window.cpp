#include "main_window.h"

#include <QAction>
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
#include "sidebar/annotations_panel.h"

namespace syodep {

namespace {

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

    m_core = new CoreController(this);
    m_canvas = new CanvasWidget(m_core, this);
    m_canvas->setBackgroundColor(m_core->backgroundColor());
    m_canvas->setFocusColor(m_core->focusColor());
    m_canvas->setVisualColor(m_core->visualColor());
    m_canvas->setHighlightColor(m_core->highlightColor());
    setCentralWidget(m_canvas);

    m_sidebarDock = new QDockWidget(tr("Highlights"), this);
    m_sidebarDock->setObjectName(QStringLiteral("annotationDock"));
    m_sidebarDock->setAllowedAreas(Qt::RightDockWidgetArea);
    m_sidebarDock->setFeatures(QDockWidget::DockWidgetClosable);
    m_annotationSidebar = new AnnotationSidebar(m_core, m_sidebarDock);
    m_sidebarDock->setWidget(m_annotationSidebar);
    addDockWidget(Qt::RightDockWidgetArea, m_sidebarDock);
    resizeDocks({m_sidebarDock}, {340}, Qt::Horizontal);
    sanitizeDockState();
    m_activeSidebarPage = SidebarPage::Highlights;

    m_highlightsAction = new QAction(tr("&Highlights"), this);
    m_highlightsAction->setCheckable(true);
    m_annotationsAction = new QAction(tr("&Annotations"), this);
    m_annotationsAction->setCheckable(true);
    connect(m_highlightsAction, &QAction::triggered, this, [this]() {
        toggleSidebarPage(SidebarPage::Highlights);
    });
    connect(m_annotationsAction, &QAction::triggered, this, [this]() {
        toggleSidebarPage(SidebarPage::Annotations);
    });

    QMenu *fileMenu = menuBar()->addMenu(tr("&File"));
    fileMenu->addAction(m_annotationSidebar->exportAction());
    fileMenu->addAction(m_annotationSidebar->annotationsExportAction());

    QMenu *viewMenu = menuBar()->addMenu(tr("&View"));
    viewMenu->addAction(m_highlightsAction);
    viewMenu->addAction(m_annotationsAction);

    connect(m_sidebarDock, &QDockWidget::visibilityChanged, this, [this](bool visible) {
        if (!visible && isVisible())
            focusCanvas();
        updateSidebarActions();
    });
    connect(m_annotationSidebar, &AnnotationSidebar::focusCanvasRequested,
            this, [this]() {
                m_annotationSidebar->clearPendingKeys();
                focusCanvas();
            });
    connect(m_core, &CoreController::toggleHighlightsSidebarRequested,
            this, &MainWindow::toggleHighlightsSidebar);
    connect(m_core, &CoreController::toggleAnnotationsSidebarRequested,
            this, &MainWindow::toggleAnnotationsSidebar);
    connect(m_core, &CoreController::createTextAnnotationRequested,
            this, &MainWindow::openAnnotationCreation);

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

    // Start with the dock closed; openDocument also hides it so a newly
    // opened file always begins with canvas focus and no sidebar.
    hideSidebar();
    refreshStatus();
}

void MainWindow::sanitizeDockState()
{
    if (!m_sidebarDock)
        return;
    if (m_sidebarDock->isFloating())
        m_sidebarDock->setFloating(false);
    if (dockWidgetArea(m_sidebarDock) != Qt::RightDockWidgetArea)
        addDockWidget(Qt::RightDockWidgetArea, m_sidebarDock);
}

void MainWindow::toggleSidebarPage(SidebarPage requested)
{
    if (!m_sidebarDock)
        return;
    if (!m_sidebarDock->isVisible()) {
        showSidebarPage(requested);
        return;
    }
    if (visibleSidebarPage() == requested) {
        hideSidebar();
        return;
    }
    showSidebarPage(requested);
}

void MainWindow::showSidebarPage(SidebarPage page)
{
    if (!m_sidebarDock || !m_annotationSidebar)
        return;
    m_activeSidebarPage = page;
    m_annotationSidebar->showPage(page);
    m_sidebarDock->setWindowTitle(page == SidebarPage::Highlights
                                      ? tr("Highlights")
                                      : tr("Annotations"));
    m_sidebarDock->setVisible(true);
    sanitizeDockState();
    m_annotationSidebar->focusActivePage();
    updateSidebarActions();
}

void MainWindow::hideSidebar()
{
    if (!m_sidebarDock)
        return;
    if (m_annotationSidebar)
        m_annotationSidebar->clearPendingKeys();
    m_sidebarDock->setVisible(false);
    focusCanvas();
    updateSidebarActions();
}

void MainWindow::openAnnotationCreation()
{
    if (!m_sidebarDock || !m_annotationSidebar)
        return;
    m_activeSidebarPage = SidebarPage::Annotations;
    m_annotationSidebar->showPage(SidebarPage::Annotations);
    m_sidebarDock->setWindowTitle(tr("Annotations"));
    m_sidebarDock->setVisible(true);
    sanitizeDockState();
    m_annotationSidebar->beginAnnotationCreation();
    updateSidebarActions();
}

void MainWindow::toggleHighlightsSidebar()
{
    toggleSidebarPage(SidebarPage::Highlights);
}

void MainWindow::toggleAnnotationsSidebar()
{
    toggleSidebarPage(SidebarPage::Annotations);
}

std::optional<SidebarPage> MainWindow::visibleSidebarPage() const
{
    if (!m_sidebarDock || !m_sidebarDock->isVisible())
        return std::nullopt;
    return m_activeSidebarPage;
}

void MainWindow::updateSidebarActions()
{
    const auto visible = visibleSidebarPage();
    if (m_highlightsAction)
        m_highlightsAction->setChecked(visible == SidebarPage::Highlights);
    if (m_annotationsAction)
        m_annotationsAction->setChecked(visible == SidebarPage::Annotations);
}

void MainWindow::focusCanvas()
{
    if (m_annotationSidebar)
        m_annotationSidebar->clearPendingKeys();
    if (m_canvas)
        m_canvas->setFocus(Qt::OtherFocusReason);
}

bool MainWindow::openDocument(const QString &path)
{
    if (m_annotationSidebar
        && !m_annotationSidebar->confirmDiscardDirty(tr("opening another document"))) {
        return false;
    }
    if (!m_core->openDocument(path))
        return false;
    // A newly opened file always starts with the sidebar closed.
    hideSidebar();
    return true;
}

bool MainWindow::startFullscreen() const
{
    return m_core && m_core->startFullscreen();
}

void MainWindow::dragEnterEvent(QDragEnterEvent *event)
{
    if (!droppablePdfs(event->mimeData()).isEmpty())
        event->acceptProposedAction();
}

void MainWindow::dropEvent(QDropEvent *event)
{
    const QStringList paths = droppablePdfs(event->mimeData());
    if (paths.isEmpty())
        return;
    event->acceptProposedAction();

    openDocument(paths.first());
    if (paths.size() > 1) {
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

    if (m_annotationSidebar
        && !m_annotationSidebar->confirmDiscardDirty(tr("closing the application"))) {
        return false;
    }

    if (!m_core->hasUnsavedHighlights()) {
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
        return false;

    if (!quit) {
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
        close();
}

void MainWindow::showOpenDialog()
{
    if (m_annotationSidebar
        && !m_annotationSidebar->confirmDiscardDirty(tr("opening another document"))) {
        return;
    }
    const QString start = m_core->openDirectory();
    const QString path = QFileDialog::getOpenFileName(
        this, tr("Open PDF"), start, tr("PDF documents (*.pdf);;All files (*)"));
    if (!path.isEmpty())
        openDocument(path);
}

} // namespace syodep
