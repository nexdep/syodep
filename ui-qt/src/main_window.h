// Main application window: canvas + status line + native file dialog.
// Owns the core (SyoApp) handle. Contains no document logic.
#pragma once

#include <QLabel>
#include <QMainWindow>

#include "syodep_ffi.h"

namespace syodep {

class CanvasWidget;

class MainWindow : public QMainWindow
{
    Q_OBJECT
public:
    explicit MainWindow(QWidget *parent = nullptr);
    ~MainWindow() override;

    // Returns false (and shows the error in the status line) on failure.
    bool openDocument(const QString &path);

private slots:
    void refreshStatus();
    void showOpenDialog();

private:
    SyoApp *m_app = nullptr;
    CanvasWidget *m_canvas = nullptr;
    QLabel *m_status = nullptr;
};

} // namespace syodep
