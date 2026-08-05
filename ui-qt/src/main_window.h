// Main application window: canvas + status line + annotation dock + dialogs.
// Composes the UI and manages dialogs; CoreController owns the SyoApp*.
#pragma once

#include <QLabel>
#include <QMainWindow>

#include <optional>

#include "sidebar/annotation_sidebar.h"

class QAction;
class QCloseEvent;
class QDockWidget;
class QResizeEvent;

namespace syodep {

class CanvasWidget;
class CoreController;
class KeybindingsOverlay;

class MainWindow : public QMainWindow
{
    Q_OBJECT
public:
    explicit MainWindow(QWidget *parent = nullptr);
    ~MainWindow() override = default;

    bool openDocument(const QString &path);

    // Whether the shell should open this window fullscreen.
    bool startFullscreen() const;
    // Whether the Highlights sidebar starts open.
    bool startSidebarOpen() const;

    AnnotationSidebar *annotationSidebar() const { return m_annotationSidebar; }
    QDockWidget *annotationsDock() const { return m_sidebarDock; }
    QAction *highlightsToggleAction() const { return m_highlightsAction; }
    QAction *annotationsToggleAction() const { return m_annotationsAction; }
    KeybindingsOverlay *keybindingsOverlay() const { return m_keybindingsOverlay; }

    // Single read model for which sidebar page is visible (nullopt when hidden).
    std::optional<SidebarPage> visibleSidebarPage() const;

public slots:
    void toggleSidebarPage(SidebarPage page);
    void showSidebarPage(SidebarPage page);
    void hideSidebar();
    void openAnnotationCreation();
    void toggleHighlightsSidebar();
    void toggleAnnotationsSidebar();
    void focusCanvas();

protected:
    void dragEnterEvent(QDragEnterEvent *event) override;
    void dropEvent(QDropEvent *event) override;
    void closeEvent(QCloseEvent *event) override;
    void resizeEvent(QResizeEvent *event) override;

private slots:
    void refreshStatus();
    void showOpenDialog();
    void onConfirmQuitRequested();
    void updateSidebarActions();
    void syncKeybindingsOverlay();

private:
    bool confirmQuit();
    void sanitizeDockState();
    // Apply [window] start_sidebar_open: show Highlights or hide the dock.
    void applyStartSidebarPreference();

    CoreController *m_core = nullptr;
    CanvasWidget *m_canvas = nullptr;
    AnnotationSidebar *m_annotationSidebar = nullptr;
    KeybindingsOverlay *m_keybindingsOverlay = nullptr;
    QDockWidget *m_sidebarDock = nullptr;
    QAction *m_highlightsAction = nullptr;
    QAction *m_annotationsAction = nullptr;
    QLabel *m_status = nullptr;
    SidebarPage m_activeSidebarPage = SidebarPage::Highlights;
    bool m_closeConfirmed = false;
};

} // namespace syodep
