// Main application window: canvas + status line + native file dialog.
// Owns the core (SyoApp) handle. Contains no document logic.
#pragma once

#include <QLabel>
#include <QMainWindow>

#include "syodep_ffi.h"

class QCloseEvent;

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

protected:
    // Dropping a PDF onto the window opens it. The canvas fills the window but
    // does not accept drops, so Qt delivers these to the window instead.
    void dragEnterEvent(QDragEnterEvent *event) override;
    void dropEvent(QDropEvent *event) override;
    // Mirrors the confirm-before-quit prompt `<leader>q` triggers, so the
    // window's own close button/Alt+F4 cannot bypass it.
    void closeEvent(QCloseEvent *event) override;

private slots:
    void refreshStatus();
    void showOpenDialog();
    void onConfirmQuitRequested();

private:
    // Shared by closeEvent and onConfirmQuitRequested: asks Save & Quit /
    // Discard & Quit / Cancel if there are unsaved highlights, and carries out
    // the answer. Returns true when the window may close now.
    bool confirmQuit();

    SyoApp *m_app = nullptr;
    CanvasWidget *m_canvas = nullptr;
    QLabel *m_status = nullptr;
    // Set once confirmQuit() has resolved (nothing to confirm, or the user
    // answered). Guards against onConfirmQuitRequested() -> close() ->
    // closeEvent() re-asking a second time after "Discard & Quit", since
    // discarding does not clear the core's unsaved-highlights state.
    bool m_closeConfirmed = false;
};

} // namespace syodep
