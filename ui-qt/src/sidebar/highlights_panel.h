// Highlights panel: the highlight list page of the annotation sidebar.
// Presentation only — document truth stays in the Rust core via CoreController.
//
// The list is driven from the keyboard the way the canvas is: j/k to move,
// gg/G to jump to the ends, Enter to go to the highlight, dd or Delete to
// remove it, y/Y to copy. Those keys are handled here rather than by the core
// because they act on a shell-owned selection, and they are consumed here so
// they cannot fall through to the canvas and move the caret instead.
#pragma once

#include <QTimer>
#include <QWidget>

#include <optional>

#include "sidebar/sidebar_list_keys.h"

class QAction;
class QListView;
class QModelIndex;
class QShowEvent;
class QStackedWidget;

namespace syodep {

class CoreController;
class HighlightListModel;
class HighlightDelegate;

// Write `markdown` to `path` through a QSaveFile, so an interrupted or failed
// write leaves whatever was already there intact. Returns false and fills
// `error` (when non-null) on failure. Free-standing because the smoke test
// exercises the write without being able to open a file dialog.
bool writeHighlightsMarkdown(const QString &path, const QString &markdown, QString *error);

class HighlightsPanel final : public QWidget
{
    Q_OBJECT

public:
    enum class ContentState {
        NoDocument,
        EmptyHighlights,
        HighlightList
    };

    explicit HighlightsPanel(CoreController *core, QWidget *parent = nullptr);

    // force=true always refetches (documentChanged). Otherwise skip when the
    // controller revision matches the model revision.
    void refreshAnnotations(bool force = false);

    ContentState contentState() const;
    HighlightListModel *model() const { return m_model; }
    QListView *listView() const { return m_listView; }

    // Owned here so the sidebar context menu and the window's menu bar trigger
    // one action with one enabled state. The window adds it to a menu, which is
    // also what keeps its shortcut alive while the dock is hidden.
    QAction *exportAction() const { return m_exportAction; }

    // Give the list the keyboard. Selects the first row when nothing is
    // selected, so the very next `j` has somewhere to go.
    void focusList();
    void clearPendingKey();

public slots:
    // File dialog + write. A no-op (with a status message) when there is
    // nothing to export: an empty file would be a worse answer than none.
    void exportHighlightsToFile();
    void deleteSelectedHighlight();

signals:
    // Escape in the list, or a deletion that emptied it: the canvas should
    // take the keyboard back. The window owns the canvas, so it decides.
    void focusCanvasRequested();

protected:
    void showEvent(QShowEvent *event) override;
    // Keyboard handling for m_listView, installed as an event filter so the
    // view keeps its own scrolling and accessibility behaviour.
    bool eventFilter(QObject *watched, QEvent *event) override;

private slots:
    void onDocumentChanged();
    void onAnnotationsChanged();
    void onActivated(const QModelIndex &index);
    void onCustomContextMenu(const QPoint &pos);
    void revealCurrent();
    void copyHighlightedText();
    void copyAsMarkdown();
    void copyAllAsMarkdown();
    void updateActionState();

private:
    enum StackPage {
        NoDocumentPage = 0,
        EmptyHighlightsPage = 1,
        ListPage = 2
    };

    void revealIndex(const QModelIndex &index);
    void updateEmptyState();
    void setupActions();
    std::optional<qint64> selectedId() const;
    QModelIndex selectedIndex() const;
    void selectRow(int row);
    void selectRowById(qint64 id);
    // True when the key was a sidebar key and must not travel further.
    bool handleListKey(QKeyEvent *event);

    CoreController *m_core = nullptr; // non-owning
    HighlightListModel *m_model = nullptr;
    HighlightDelegate *m_delegate = nullptr;

    QStackedWidget *m_stack = nullptr;
    QListView *m_listView = nullptr;

    QAction *m_revealAction = nullptr;
    QAction *m_copyTextAction = nullptr;
    QAction *m_copyMarkdownAction = nullptr;
    QAction *m_copyAllAction = nullptr;
    QAction *m_deleteAction = nullptr;
    QAction *m_exportAction = nullptr;

    // The first key of a two-key sequence, once it has been typed and is
    // waiting for its partner. Mirrors the core's pending-input state, on the
    // core's timeout, so `g` or `d` alone expires instead of arming forever.
    SidebarPendingKey m_pendingKey = SidebarPendingKey::None;
    QTimer m_pendingKeyTimer;

    bool m_initialRefreshDone = false;
    // Row to fall back to when the selected highlight disappears: after a
    // delete the user is looking at a position in the list, not at an id.
    int m_rowAfterDelete = -1;
};

} // namespace syodep
