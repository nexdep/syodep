#include "sidebar/annotation_sidebar.h"

#include <QAction>
#include <QClipboard>
#include <QGuiApplication>
#include <QItemSelectionModel>
#include <QKeySequence>
#include <QLabel>
#include <QListView>
#include <QMenu>
#include <QMessageBox>
#include <QPushButton>
#include <QShowEvent>
#include <QSignalBlocker>
#include <QSplitter>
#include <QStackedWidget>
#include <QVBoxLayout>

#include "core_controller.h"
#include "sidebar/highlight_comment_editor.h"
#include "sidebar/highlight_delegate.h"
#include "sidebar/highlight_list_model.h"

namespace syodep {

namespace {

QWidget *makeEmptyPage(const QString &title, const QString &subtitle)
{
    auto *page = new QWidget;
    auto *layout = new QVBoxLayout(page);
    layout->setContentsMargins(24, 24, 24, 24);
    layout->addStretch(1);

    auto *titleLabel = new QLabel(title, page);
    titleLabel->setAlignment(Qt::AlignCenter);
    titleLabel->setWordWrap(true);
    QFont titleFont = titleLabel->font();
    titleFont.setPointSizeF(titleFont.pointSizeF() + 1.0);
    titleFont.setBold(true);
    titleLabel->setFont(titleFont);
    titleLabel->setForegroundRole(QPalette::WindowText);
    layout->addWidget(titleLabel);

    auto *subLabel = new QLabel(subtitle, page);
    subLabel->setAlignment(Qt::AlignCenter);
    subLabel->setWordWrap(true);
    subLabel->setForegroundRole(QPalette::PlaceholderText);
    layout->addWidget(subLabel);

    layout->addStretch(1);
    return page;
}

} // namespace

AnnotationSidebar::AnnotationSidebar(CoreController *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
{
    Q_ASSERT(m_core);

    m_model = new HighlightListModel(this);
    m_delegate = new HighlightDelegate(this);

    m_stack = new QStackedWidget(this);
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);
    root->addWidget(m_stack);

    m_stack->addWidget(makeEmptyPage(
        tr("No document open"),
        tr("Open a PDF to view its highlights.")));
    m_stack->addWidget(makeEmptyPage(
        tr("No highlights"),
        tr("Create a highlight to see it here.")));

    m_listPage = new QWidget(m_stack);
    auto *listLayout = new QVBoxLayout(m_listPage);
    listLayout->setContentsMargins(0, 0, 0, 0);
    listLayout->setSpacing(0);

    m_splitter = new QSplitter(Qt::Vertical, m_listPage);
    m_listView = new QListView(m_splitter);
    m_listView->setModel(m_model);
    m_listView->setItemDelegate(m_delegate);
    m_listView->setSelectionMode(QAbstractItemView::SingleSelection);
    m_listView->setSelectionBehavior(QAbstractItemView::SelectRows);
    m_listView->setVerticalScrollMode(QAbstractItemView::ScrollPerPixel);
    m_listView->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    m_listView->setWordWrap(true);
    m_listView->setUniformItemSizes(false);
    m_listView->setEditTriggers(QAbstractItemView::NoEditTriggers);
    m_listView->setContextMenuPolicy(Qt::CustomContextMenu);
    m_listView->setAlternatingRowColors(false);
    m_listView->setMouseTracking(true);
    m_listView->viewport()->setAttribute(Qt::WA_Hover, true);

    m_commentEditor = new HighlightCommentEditor(m_splitter);
    m_splitter->addWidget(m_listView);
    m_splitter->addWidget(m_commentEditor);
    m_splitter->setStretchFactor(0, 3);
    m_splitter->setStretchFactor(1, 2);
    m_splitter->setChildrenCollapsible(false);

    listLayout->addWidget(m_splitter);
    m_stack->addWidget(m_listPage);

    setupActions();

    connect(m_core, &CoreController::documentChanged,
            this, &AnnotationSidebar::onDocumentChanged);
    connect(m_core, &CoreController::annotationsChanged,
            this, &AnnotationSidebar::onAnnotationsChanged);

    connect(m_listView, &QListView::activated,
            this, &AnnotationSidebar::onActivated);
    connect(m_listView, &QListView::customContextMenuRequested,
            this, &AnnotationSidebar::onCustomContextMenu);
    connect(m_listView->selectionModel(), &QItemSelectionModel::currentChanged,
            this, &AnnotationSidebar::onListCurrentChanged);

    connect(m_commentEditor, &HighlightCommentEditor::saveRequested,
            this, &AnnotationSidebar::onSaveComment);
    connect(m_commentEditor, &HighlightCommentEditor::dirtyStateChanged,
            this, &AnnotationSidebar::updateActionState);

    refreshAnnotations(true);
}

void AnnotationSidebar::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    if (!m_initialRefreshDone) {
        m_initialRefreshDone = true;
        refreshAnnotations(true);
    }
}

void AnnotationSidebar::onDocumentChanged()
{
    m_commentEditor->clearHighlight();
    refreshAnnotations(true);
}

void AnnotationSidebar::onAnnotationsChanged()
{
    refreshAnnotations(false);
}

void AnnotationSidebar::refreshAnnotations(bool force)
{
    if (!m_core)
        return;

    const quint64 revision = m_core->annotationRevision();
    if (!force && revision == m_model->revision()) {
        updateEmptyState();
        return;
    }

    const std::optional<qint64> previousId = selectedId();
    const bool editorDirty = m_commentEditor->isDirty();
    const qint64 editorId = m_commentEditor->highlightId();
    const QString dirtyDraft = editorDirty ? m_commentEditor->markdown() : QString();
    const QString dirtySaved = editorDirty ? m_commentEditor->savedMarkdown() : QString();

    HighlightSnapshot snapshot;
    if (m_core->hasDocument())
        snapshot = m_core->highlightSnapshot();

    m_suppressSelectionPrompt = true;
    m_model->setSnapshot(std::move(snapshot));

    qint64 restoreId = 0;
    if (previousId && m_model->rowForId(*previousId))
        restoreId = *previousId;
    else if (m_pendingSavedId != 0 && m_model->rowForId(m_pendingSavedId))
        restoreId = m_pendingSavedId;

    if (restoreId != 0)
        selectRowById(restoreId);
    else {
        m_listView->clearSelection();
        m_listView->setCurrentIndex(QModelIndex());
    }
    m_suppressSelectionPrompt = false;

    if (editorDirty && editorId != 0) {
        if (const std::optional<int> row = m_model->rowForId(editorId)) {
            const HighlightListItem *item = m_model->itemAt(*row);
            if (item) {
                m_commentEditor->reloadPreservingDraft(*item, dirtySaved, dirtyDraft);
                selectRowById(editorId);
            }
        } else {
            QMessageBox::information(
                this,
                tr("Comment"),
                tr("The highlight for this comment is no longer available. "
                   "The unsaved draft was discarded."));
            m_commentEditor->clearHighlight();
        }
    } else {
        loadEditorForSelection();
    }

    m_pendingSavedId = 0;
    updateEmptyState();
    updateActionState();
}

void AnnotationSidebar::updateEmptyState()
{
    if (!m_core || !m_core->hasDocument()) {
        m_stack->setCurrentIndex(NoDocumentPage);
        return;
    }
    if (m_model->rowCount() == 0) {
        m_stack->setCurrentIndex(EmptyHighlightsPage);
        return;
    }
    m_stack->setCurrentIndex(ListPage);
}

AnnotationSidebar::ContentState AnnotationSidebar::contentState() const
{
    switch (m_stack->currentIndex()) {
    case EmptyHighlightsPage:
        return ContentState::EmptyHighlights;
    case ListPage:
        return ContentState::HighlightList;
    case NoDocumentPage:
    default:
        return ContentState::NoDocument;
    }
}

void AnnotationSidebar::setupActions()
{
    m_revealAction = new QAction(tr("Go to highlight"), this);
    connect(m_revealAction, &QAction::triggered, this, &AnnotationSidebar::revealCurrent);

    m_copyTextAction = new QAction(tr("Copy highlighted text"), this);
    m_copyTextAction->setShortcut(QKeySequence::Copy);
    m_copyTextAction->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    addAction(m_copyTextAction);
    connect(m_copyTextAction, &QAction::triggered, this, &AnnotationSidebar::copyHighlightedText);

    m_copyMarkdownAction = new QAction(tr("Copy as Markdown"), this);
    m_copyMarkdownAction->setShortcut(
        QKeySequence(QStringLiteral("Ctrl+Shift+C")));
    m_copyMarkdownAction->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    addAction(m_copyMarkdownAction);
    connect(m_copyMarkdownAction, &QAction::triggered, this, &AnnotationSidebar::copyAsMarkdown);

    m_copyCommentAction = new QAction(tr("Copy comment Markdown"), this);
    connect(m_copyCommentAction, &QAction::triggered,
            this, &AnnotationSidebar::copyCommentMarkdown);

    m_copyAllAction = new QAction(tr("Copy all highlights as Markdown"), this);
    connect(m_copyAllAction, &QAction::triggered, this, &AnnotationSidebar::copyAllAsMarkdown);

    m_listView->addAction(m_copyTextAction);
    m_listView->addAction(m_copyMarkdownAction);

    updateActionState();
}

void AnnotationSidebar::updateActionState()
{
    const bool hasSelection = selectedIndex().isValid();
    const bool hasDocument = m_core && m_core->hasDocument();
    const bool hasRows = m_model && m_model->rowCount() > 0;
    const bool hasComment = hasSelection
        && (m_commentEditor->isDirty()
            || (!m_commentEditor->markdown().isEmpty())
            || selectedIndex().data(HighlightListModel::HasNoteRole).toBool());

    m_revealAction->setEnabled(hasSelection);
    m_copyTextAction->setEnabled(hasSelection);
    m_copyMarkdownAction->setEnabled(hasSelection);
    m_copyCommentAction->setEnabled(hasComment);
    m_copyAllAction->setEnabled(hasDocument && hasRows);
}

QModelIndex AnnotationSidebar::selectedIndex() const
{
    if (!m_listView || !m_listView->selectionModel())
        return {};
    return m_listView->selectionModel()->currentIndex();
}

std::optional<qint64> AnnotationSidebar::selectedId() const
{
    const QModelIndex idx = selectedIndex();
    if (!idx.isValid())
        return std::nullopt;
    bool ok = false;
    const qint64 id = idx.data(HighlightListModel::IdRole).toLongLong(&ok);
    if (!ok)
        return std::nullopt;
    return id;
}

void AnnotationSidebar::selectRowById(qint64 id)
{
    if (const std::optional<int> row = m_model->rowForId(id)) {
        const QModelIndex idx = m_model->index(*row, 0);
        const bool prev = m_suppressSelectionPrompt;
        m_suppressSelectionPrompt = true;
        m_listView->setCurrentIndex(idx);
        m_listView->selectionModel()->select(
            idx, QItemSelectionModel::ClearAndSelect | QItemSelectionModel::Rows);
        m_suppressSelectionPrompt = prev;
    }
}

bool AnnotationSidebar::resolveDirtyEditor()
{
    if (!m_commentEditor->isDirty())
        return true;

    QMessageBox box(this);
    box.setIcon(QMessageBox::Warning);
    box.setWindowTitle(tr("Unsaved comment"));
    box.setText(tr("The comment has unsaved changes."));
    QPushButton *saveBtn = box.addButton(tr("Save"), QMessageBox::AcceptRole);
    QPushButton *discardBtn = box.addButton(tr("Discard"), QMessageBox::DestructiveRole);
    box.addButton(tr("Cancel"), QMessageBox::RejectRole);
    box.setDefaultButton(saveBtn);
    box.exec();

    if (box.clickedButton() == saveBtn) {
        const qint64 id = m_commentEditor->highlightId();
        const QString body = m_commentEditor->markdown();
        if (!m_core->setHighlightNote(id, body))
            return false;
        const QString canonical = body.trimmed().isEmpty() ? QString() : body;
        m_commentEditor->markSaved(canonical);
        m_pendingSavedId = id;
        return true;
    }
    if (box.clickedButton() == discardBtn) {
        m_commentEditor->revert();
        return true;
    }
    return false;
}

bool AnnotationSidebar::prepareForDocumentChange()
{
    return resolveDirtyEditor();
}

void AnnotationSidebar::onListCurrentChanged(const QModelIndex &current,
                                             const QModelIndex &previous)
{
    Q_UNUSED(current);
    onCurrentChanged(previous);
}

void AnnotationSidebar::onCurrentChanged(const QModelIndex &previous)
{
    if (m_suppressSelectionPrompt)
        return;

    if (m_commentEditor->isDirty()) {
        const qint64 editingId = m_commentEditor->highlightId();
        bool previousMatches = false;
        if (previous.isValid()) {
            bool ok = false;
            const qint64 prevId = previous.data(HighlightListModel::IdRole).toLongLong(&ok);
            previousMatches = ok && prevId == editingId;
        }
        if (editingId != 0 && previousMatches) {
            // Selection left the dirty highlight. Prompt; cancel restores it.
            if (!resolveDirtyEditor()) {
                selectRowById(editingId);
                return;
            }
        }
    }

    loadEditorForSelection();
    updateActionState();
}

void AnnotationSidebar::loadEditorForSelection()
{
    const QModelIndex idx = selectedIndex();
    if (!idx.isValid()) {
        if (!m_commentEditor->isDirty())
            m_commentEditor->clearHighlight();
        return;
    }
    const HighlightListItem *item = m_model->itemAt(idx.row());
    if (!item) {
        if (!m_commentEditor->isDirty())
            m_commentEditor->clearHighlight();
        return;
    }
    if (m_commentEditor->isDirty() && m_commentEditor->highlightId() == item->id)
        return;
    m_commentEditor->loadHighlight(*item);
}

void AnnotationSidebar::revealIndex(const QModelIndex &index)
{
    if (!index.isValid() || !m_core)
        return;
    bool ok = false;
    const qint64 id = index.data(HighlightListModel::IdRole).toLongLong(&ok);
    if (!ok)
        return;
    m_core->revealHighlight(id);
    m_listView->setFocus(Qt::OtherFocusReason);
}

void AnnotationSidebar::onActivated(const QModelIndex &index)
{
    revealIndex(index);
}

void AnnotationSidebar::revealCurrent()
{
    revealIndex(selectedIndex());
}

void AnnotationSidebar::copyHighlightedText()
{
    const QModelIndex idx = selectedIndex();
    if (!idx.isValid())
        return;
    QGuiApplication::clipboard()->setText(idx.data(HighlightListModel::TextRole).toString());
}

void AnnotationSidebar::copyAsMarkdown()
{
    // Canonical saved export from Rust — unsaved drafts are excluded. The
    // editor shows "Unsaved changes" so the distinction is visible.
    const std::optional<qint64> id = selectedId();
    if (!id || !m_core)
        return;
    const QString markdown = m_core->highlightMarkdown(*id);
    if (markdown.isEmpty())
        return;
    QGuiApplication::clipboard()->setText(markdown);
}

void AnnotationSidebar::copyCommentMarkdown()
{
    // Current editor draft when this highlight is being edited; otherwise the
    // saved model note. Distinct from "Copy as Markdown" (canonical export).
    if (m_commentEditor->hasHighlight()
        && selectedId()
        && *selectedId() == m_commentEditor->highlightId()) {
        QGuiApplication::clipboard()->setText(m_commentEditor->markdown());
        return;
    }
    const QModelIndex idx = selectedIndex();
    if (!idx.isValid())
        return;
    QGuiApplication::clipboard()->setText(
        idx.data(HighlightListModel::NoteMarkdownRole).toString());
}

void AnnotationSidebar::copyAllAsMarkdown()
{
    if (!m_core)
        return;
    const QString markdown = m_core->allHighlightsMarkdown();
    if (markdown.isEmpty() && m_model->rowCount() > 0)
        return;
    QGuiApplication::clipboard()->setText(markdown);
}

void AnnotationSidebar::onSaveComment(qint64 highlightId, const QString &markdown)
{
    if (!m_core)
        return;
    if (!m_core->setHighlightNote(highlightId, markdown))
        return;
    const QString canonical = markdown.trimmed().isEmpty() ? QString() : markdown;
    m_commentEditor->markSaved(canonical);
    m_pendingSavedId = highlightId;
}

void AnnotationSidebar::onCustomContextMenu(const QPoint &pos)
{
    updateActionState();
    QMenu menu(this);
    menu.addAction(m_revealAction);
    menu.addSeparator();
    menu.addAction(m_copyTextAction);
    menu.addAction(m_copyMarkdownAction);
    menu.addAction(m_copyCommentAction);
    menu.addSeparator();
    menu.addAction(m_copyAllAction);
    menu.exec(m_listView->viewport()->mapToGlobal(pos));
}

} // namespace syodep
