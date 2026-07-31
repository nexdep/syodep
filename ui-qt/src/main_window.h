// Main application window: canvas + status line + highlights dock + dialogs.
// Composes the UI and manages dialogs; CoreController owns the SyoApp*.
#pragma once

#include <QLabel>
#include <QMainWindow>

class QCloseEvent;
class QDockWidget;

namespace syodep {

class AnnotationSidebar;
class CanvasWidget;
class CoreController;

class MainWindow : public QMainWindow
{
    Q_OBJECT
public:
    explicit MainWindow(QWidget *parent = nullptr);
    ~MainWindow() override = default;

    // Returns false (and shows the error in the status line) on failure.
    bool openDocument(const QString &path);

    // Exposed for smoke tests that need to assert sidebar construction.
    AnnotationSidebar *annotationSidebar() const { return m_annotationSidebar; }
    QDockWidget *annotationsDock() const { return m_annotationsDock; }

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

    CoreController *m_core = nullptr;
    CanvasWidget *m_canvas = nullptr;
    AnnotationSidebar *m_annotationSidebar = nullptr;
    QDockWidget *m_annotationsDock = nullptr;
    QLabel *m_status = nullptr;
    // Set once confirmQuit() has resolved (nothing to confirm, or the user
    // answered). Guards against onConfirmQuitRequested() -> close() ->
    // closeEvent() re-asking a second time after "Discard & Quit", since
    // discarding does not clear the core's unsaved-highlights state.
    bool m_closeConfirmed = false;
};

} // namespace syodep
