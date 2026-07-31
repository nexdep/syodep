// Annotation sidebar: highlight list + Markdown comment editor.
// Presentation only — document truth stays in the Rust core via CoreController.
#pragma once

#include <QWidget>

#include <optional>

class QListView;
class QStackedWidget;
class QSplitter;
class QAction;
class QShowEvent;
class QModelIndex;

namespace syodep {

class CoreController;
class HighlightListModel;
class HighlightDelegate;
class HighlightCommentEditor;

class AnnotationSidebar final : public QWidget
{
    Q_OBJECT

public:
    enum class ContentState {
        NoDocument,
        EmptyHighlights,
        HighlightList
    };

    explicit AnnotationSidebar(CoreController *core, QWidget *parent = nullptr);

    // force=true always refetches (documentChanged). Otherwise skip when the
    // controller revision matches the model revision.
    void refreshAnnotations(bool force = false);

    // Resolve an unsaved comment draft (Save / Discard / Cancel). Returns false
    // when the user cancels — callers must abort open/close.
    bool prepareForDocumentChange();
    bool prepareForClose() { return prepareForDocumentChange(); }

    ContentState contentState() const;
    HighlightListModel *model() const { return m_model; }
    HighlightCommentEditor *commentEditor() const { return m_commentEditor; }

protected:
    void showEvent(QShowEvent *event) override;

private slots:
    void onDocumentChanged();
    void onAnnotationsChanged();
    void onListCurrentChanged(const QModelIndex &current, const QModelIndex &previous);
    void onCurrentChanged(const QModelIndex &previous);
    void onActivated(const QModelIndex &index);
    void onCustomContextMenu(const QPoint &pos);
    void revealCurrent();
    void copyHighlightedText();
    void copyAsMarkdown();
    void copyCommentMarkdown();
    void copyAllAsMarkdown();
    void onSaveComment(qint64 highlightId, const QString &markdown);
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
    bool resolveDirtyEditor();
    void loadEditorForSelection();
    void selectRowById(qint64 id);

    CoreController *m_core = nullptr; // non-owning
    HighlightListModel *m_model = nullptr;
    HighlightDelegate *m_delegate = nullptr;
    HighlightCommentEditor *m_commentEditor = nullptr;

    QStackedWidget *m_stack = nullptr;
    QSplitter *m_splitter = nullptr;
    QListView *m_listView = nullptr;
    QWidget *m_listPage = nullptr;

    QAction *m_revealAction = nullptr;
    QAction *m_copyTextAction = nullptr;
    QAction *m_copyMarkdownAction = nullptr;
    QAction *m_copyCommentAction = nullptr;
    QAction *m_copyAllAction = nullptr;

    bool m_initialRefreshDone = false;
    bool m_suppressSelectionPrompt = false;
    // After a successful save, ignore the next annotationsChanged overwrite of
    // the editor draft for this id (already marked saved).
    qint64 m_pendingSavedId = 0;
};

} // namespace syodep
