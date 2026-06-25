#include "main_window.h"

#include <QFileDialog>
#include <QStatusBar>

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
    setCentralWidget(m_canvas);

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
