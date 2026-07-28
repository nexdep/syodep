#include "main_window.h"

#include <QDragEnterEvent>
#include <QDropEvent>
#include <QFileDialog>
#include <QFileInfo>
#include <QMimeData>
#include <QStatusBar>
#include <QUrl>

#include "canvas_widget.h"

namespace syodep {

namespace {

QString takeSyoString(char *s)
{
    if (!s)
        return {};
    const QString out = QString::fromUtf8(s);
    syo_string_free(s);
    return out;
}

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

    const QString configPath = takeSyoString(syo_default_config_path());
    const QString dbPath = takeSyoString(syo_default_db_path());
    m_app = syo_app_new(configPath.toUtf8().constData(), dbPath.toUtf8().constData());

    m_canvas = new CanvasWidget(m_app, this);
    // Overlay colours come from [view] in the config, resolved by the core.
    // Until now `background` was defined and documented but never read here.
    const auto toQColor = [](SyoColor c) { return QColor(c.r, c.g, c.b, c.a); };
    m_canvas->setBackgroundColor(toQColor(syo_app_background_color(m_app)));
    m_canvas->setFocusColor(toQColor(syo_app_focus_color(m_app)));
    m_canvas->setVisualColor(toQColor(syo_app_visual_color(m_app)));
    setCentralWidget(m_canvas);

    // The canvas covers the window but leaves acceptDrops() false, so Qt walks
    // up to the window for drag events. Only the window needs the flag.
    setAcceptDrops(true);

    m_status = new QLabel(this);
    m_status->setTextFormat(Qt::PlainText);
    statusBar()->addWidget(m_status, 1);

    connect(m_canvas, &CanvasWidget::coreStateChanged, this, &MainWindow::refreshStatus);
    connect(m_canvas, &CanvasWidget::quitRequested, this, &MainWindow::close);
    connect(m_canvas, &CanvasWidget::openFileRequested, this, &MainWindow::showOpenDialog);

    const QString warnings = takeSyoString(syo_app_startup_warnings(m_app));
    if (!warnings.isEmpty())
        statusBar()->showMessage(warnings.section(QLatin1Char('\n'), 0, 0), 10000);

    refreshStatus();
}

MainWindow::~MainWindow()
{
    syo_app_free(m_app);
}

bool MainWindow::openDocument(const QString &path)
{
    const bool ok = syo_app_open_document(m_app, path.toUtf8().constData());
    m_canvas->update();
    refreshStatus();
    return ok;
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
    m_status->setText(takeSyoString(syo_app_status_text(m_app)));
}

void MainWindow::showOpenDialog()
{
    const QString start = takeSyoString(syo_app_open_dir(m_app));
    const QString path = QFileDialog::getOpenFileName(
        this, tr("Open PDF"), start, tr("PDF documents (*.pdf);;All files (*)"));
    if (!path.isEmpty())
        openDocument(path);
}

} // namespace syodep
