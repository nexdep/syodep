// Annotations page: list + Markdown editor for independent text annotations.
#pragma once

#include <QTimer>
#include <QWidget>

#include <optional>

#include "sidebar/sidebar_list_keys.h"

class QAction;
class QListView;
class QSplitter;
class QStackedWidget;
class QShowEvent;
class QKeyEvent;

namespace syodep {

class CoreController;
class TextAnnotationListModel;
class TextAnnotationDelegate;
class TextAnnotationEditor;

bool writeTextAnnotationsMarkdown(const QString &path, const QString &markdown,
                                  QString *error);

class AnnotationsPanel final : public QWidget
{
    Q_OBJECT

public:
    enum class ContentState {
        NoDocument,
        EmptyAnnotations,
        AnnotationList
    };

    explicit AnnotationsPanel(CoreController *core, QWidget *parent = nullptr);

    void refresh(bool force = false);
    ContentState contentState() const;
    TextAnnotationListModel *model() const { return m_model; }
    QListView *listView() const { return m_listView; }
    TextAnnotationEditor *editor() const { return m_editor; }
    QAction *exportAction() const { return m_exportAction; }

    void focusList();
    void focusEditor();
    void beginCreation();
    bool isDirty() const;
    bool isCreating() const;
    bool confirmDiscardDirty(const QString &actionLabel);
    void clearPendingKey();

signals:
    void focusCanvasRequested();

protected:
    void showEvent(QShowEvent *event) override;
    bool eventFilter(QObject *watched, QEvent *event) override;

private slots:
    void onDocumentChanged();
    void onAnnotationsChanged();
    void onSelectionChanged();
    void onActivated(const QModelIndex &index);
    void onSaveRequested(qint64 annotationId, const QString &markdown);
    void onCancelCreate();
    void exportToFile();
    void deleteSelected();
    void revealCurrent();
    void editCurrent();
    void copySource();
    void copyMarkdown();
    void updateActionState();

private:
    enum StackPage {
        NoDocumentPage = 0,
        EmptyAnnotationsPage = 1,
        ListPage = 2
    };

    void updateEmptyState();
    void setupActions();
    std::optional<qint64> selectedId() const;
    QModelIndex selectedIndex() const;
    void selectRow(int row);
    void selectRowById(qint64 id);
    bool handleListKey(QKeyEvent *event);
    void loadSelectedIntoEditor();
    bool trySaveDirtyEditor();

    CoreController *m_core = nullptr;
    TextAnnotationListModel *m_model = nullptr;
    TextAnnotationDelegate *m_delegate = nullptr;
    TextAnnotationEditor *m_editor = nullptr;

    QStackedWidget *m_stack = nullptr;
    QListView *m_listView = nullptr;
    QSplitter *m_splitter = nullptr;

    QAction *m_revealAction = nullptr;
    QAction *m_editAction = nullptr;
    QAction *m_copyTextAction = nullptr;
    QAction *m_copyMarkdownAction = nullptr;
    QAction *m_deleteAction = nullptr;
    QAction *m_exportAction = nullptr;

    SidebarPendingKey m_pendingKey = SidebarPendingKey::None;
    QTimer m_pendingKeyTimer;

    bool m_initialRefreshDone = false;
    bool m_loadingSelection = false;
    int m_rowAfterDelete = -1;
    // Prefer this id after a successful create (stable restore).
    qint64 m_selectAfterRefresh = 0;
};

} // namespace syodep
