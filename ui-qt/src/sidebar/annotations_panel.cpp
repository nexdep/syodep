#include "sidebar/annotations_panel.h"

#include <QAction>
#include <QClipboard>
#include <QDir>
#include <QEvent>
#include <QFileDialog>
#include <QFileInfo>
#include <QGuiApplication>
#include <QItemSelectionModel>
#include <QKeyEvent>
#include <QKeySequence>
#include <QLabel>
#include <QListView>
#include <QMainWindow>
#include <QMessageBox>
#include <QPushButton>
#include <QSaveFile>
#include <QShowEvent>
#include <QSplitter>
#include <QStackedWidget>
#include <QStatusBar>
#include <QVBoxLayout>

#include "core_controller.h"
#include "sidebar/sidebar_list_keys.h"
#include "sidebar/text_annotation_delegate.h"
#include "sidebar/text_annotation_editor.h"
#include "sidebar/text_annotation_list_model.h"

namespace syodep {

namespace {

constexpr int kFallbackSequenceTimeoutMs = 600;

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

QString pageLabelForIndex(const QModelIndex &index)
{
    const qsizetype first =
        index.data(TextAnnotationListModel::FirstPageRole).toLongLong();
    const qsizetype last =
        index.data(TextAnnotationListModel::LastPageRole).toLongLong();
    if (first == last)
        return QObject::tr("Page %1").arg(first + 1);
    return QObject::tr("Pages %1–%2").arg(first + 1).arg(last + 1);
}

QString describeSaveFileError(const QSaveFile &file, const QString &phase)
{
    const QString detail = file.errorString();
    if (detail.isEmpty())
        return phase;
    return QObject::tr("%1: %2").arg(phase, detail);
}

void showExportStatus(QWidget *widget, const QString &path)
{
    if (auto *main = qobject_cast<QMainWindow *>(widget->window()))
        main->statusBar()->showMessage(
            QObject::tr("Exported to %1").arg(path), 5000);
}

} // namespace

bool writeTextAnnotationsMarkdown(const QString &path, const QString &markdown,
                                  QString *error)
{
    QSaveFile file(path);
    // Binary (no QIODevice::Text): keep LF newlines on Windows so Markdown
    // export matches the core's `\n` and smoke round-trips byte-for-byte.
    if (!file.open(QIODevice::WriteOnly)) {
        if (error)
            *error = describeSaveFileError(file, QObject::tr("cannot open for writing"));
        return false;
    }
    QByteArray payload = markdown.toUtf8();
    if (!payload.endsWith('\n'))
        payload.append('\n');
    if (file.write(payload) != payload.size()) {
        if (error)
            *error = describeSaveFileError(file, QObject::tr("write failed"));
        return false;
    }
    if (!file.commit()) {
        if (error)
            *error = describeSaveFileError(file, QObject::tr("commit failed"));
        return false;
    }
    return true;
}

AnnotationsPanel::AnnotationsPanel(CoreController *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
{
    Q_ASSERT(m_core);
    setFocusPolicy(Qt::StrongFocus);

    m_model = new TextAnnotationListModel(this);
    m_delegate = new TextAnnotationDelegate(this);

    m_stack = new QStackedWidget(this);
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);
    root->addWidget(m_stack);

    m_stack->addWidget(makeEmptyPage(
        tr("No document open"),
        tr("Open a PDF to view its text annotations.")));
    m_stack->addWidget(makeEmptyPage(
        tr("No annotations"),
        tr("Select text and press n to add a Markdown annotation.")));

    m_splitter = new QSplitter(Qt::Horizontal, m_stack);
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
    m_listView->setAlternatingRowColors(false);
    m_listView->setMouseTracking(true);
    m_listView->viewport()->setAttribute(Qt::WA_Hover, true);
    m_listView->installEventFilter(this);

    m_editor = new TextAnnotationEditor(m_splitter);
    m_splitter->addWidget(m_listView);
    m_splitter->addWidget(m_editor);
    m_splitter->setStretchFactor(0, 1);
    m_splitter->setStretchFactor(1, 1);
    m_stack->addWidget(m_splitter);

    m_pendingKeyTimer.setSingleShot(true);
    const int timeout = m_core->keyTimeoutMs();
    m_pendingKeyTimer.setInterval(timeout > 0 ? timeout : kFallbackSequenceTimeoutMs);
    connect(&m_pendingKeyTimer, &QTimer::timeout, this, &AnnotationsPanel::clearPendingKey);

    setupActions();

    connect(m_core, &CoreController::documentChanged,
            this, &AnnotationsPanel::onDocumentChanged);
    connect(m_core, &CoreController::annotationsChanged,
            this, &AnnotationsPanel::onAnnotationsChanged);

    connect(m_listView, &QListView::activated,
            this, &AnnotationsPanel::onActivated);
    connect(m_listView->selectionModel(), &QItemSelectionModel::currentChanged,
            this, [this](const QModelIndex &, const QModelIndex &) {
                onSelectionChanged();
            });

    connect(m_editor, &TextAnnotationEditor::saveRequested,
            this, &AnnotationsPanel::onSaveRequested);
    connect(m_editor, &TextAnnotationEditor::cancelCreateRequested,
            this, &AnnotationsPanel::onCancelCreate);
    connect(m_editor, &TextAnnotationEditor::escapeToListRequested,
            this, &AnnotationsPanel::onEditorEscapeToList);
    connect(m_editor, &TextAnnotationEditor::dirtyStateChanged,
            this, [this](bool) { updateActionState(); });

    refresh(true);
}

void AnnotationsPanel::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    if (!m_initialRefreshDone) {
        m_initialRefreshDone = true;
        refresh(true);
    }
}

void AnnotationsPanel::onDocumentChanged()
{
    m_rowAfterDelete = -1;
    m_selectAfterRefresh = 0;
    clearPendingKey();
    if (m_editor && m_editor->isCreating()) {
        m_core->cancelPendingAnnotation();
        m_editor->clearAnnotation();
    }
    refresh(true);
}

void AnnotationsPanel::onAnnotationsChanged()
{
    refresh(false);
}

void AnnotationsPanel::refresh(bool force)
{
    if (!m_core)
        return;

    const quint64 revision = m_core->annotationRevision();
    if (!force && revision == m_model->revision()) {
        updateEmptyState();
        return;
    }

    const std::optional<qint64> previousId = selectedId();

    const bool editorCreating = m_editor && m_editor->isCreating();
    const bool editorDirty = m_editor && m_editor->isDirty();
    const qint64 editorId = m_editor ? m_editor->annotationId() : 0;
    const QString editorDraft = editorDirty ? m_editor->markdown() : QString();

    TextAnnotationSnapshot snapshot;
    if (m_core->hasDocument())
        snapshot = m_core->textAnnotationSnapshot();

    m_model->setSnapshot(std::move(snapshot));

    m_loadingSelection = true;
    if (m_selectAfterRefresh != 0 && m_model->rowForId(m_selectAfterRefresh) >= 0) {
        selectRowById(m_selectAfterRefresh);
    } else if (previousId && m_model->rowForId(*previousId) >= 0) {
        selectRowById(*previousId);
    } else if (m_rowAfterDelete >= 0 && m_model->rowCount() > 0) {
        selectRow(qMin(m_rowAfterDelete, m_model->rowCount() - 1));
    } else if (!editorCreating) {
        m_listView->clearSelection();
        m_listView->setCurrentIndex(QModelIndex());
    }
    m_loadingSelection = false;
    m_rowAfterDelete = -1;
    m_selectAfterRefresh = 0;

    if (editorCreating) {
        // Creation UI stays as-is across snapshot updates.
    } else if (editorDirty && editorId != 0) {
        if (const int row = m_model->rowForId(editorId); row >= 0) {
            const TextAnnotationListItem item = m_model->itemAt(row);
            m_editor->reloadPreservingDraft(item, item.bodyMarkdown, editorDraft);
        } else {
            m_editor->clearAnnotation();
        }
    } else {
        loadSelectedIntoEditor();
    }

    updateEmptyState();
    updateActionState();
}

void AnnotationsPanel::updateEmptyState()
{
    if (!m_core || !m_core->hasDocument()) {
        m_stack->setCurrentIndex(NoDocumentPage);
        return;
    }
    if (m_model->rowCount() == 0 && !(m_editor && m_editor->isCreating())) {
        m_stack->setCurrentIndex(EmptyAnnotationsPage);
        return;
    }
    m_stack->setCurrentIndex(ListPage);
}

AnnotationsPanel::ContentState AnnotationsPanel::contentState() const
{
    switch (m_stack->currentIndex()) {
    case EmptyAnnotationsPage:
        return ContentState::EmptyAnnotations;
    case ListPage:
        return ContentState::AnnotationList;
    case NoDocumentPage:
    default:
        return ContentState::NoDocument;
    }
}

void AnnotationsPanel::setupActions()
{
    m_revealAction = new QAction(tr("Go to annotation"), this);
    connect(m_revealAction, &QAction::triggered, this, &AnnotationsPanel::revealCurrent);

    m_editAction = new QAction(tr("Edit annotation"), this);
    connect(m_editAction, &QAction::triggered, this, &AnnotationsPanel::editCurrent);

    m_copyTextAction = new QAction(tr("Copy source text"), this);
    m_copyTextAction->setShortcut(QKeySequence::Copy);
    m_copyTextAction->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    addAction(m_copyTextAction);
    connect(m_copyTextAction, &QAction::triggered, this, &AnnotationsPanel::copySource);

    m_copyMarkdownAction = new QAction(tr("Copy as Markdown"), this);
    m_copyMarkdownAction->setShortcut(
        QKeySequence(QStringLiteral("Ctrl+Shift+C")));
    m_copyMarkdownAction->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    addAction(m_copyMarkdownAction);
    connect(m_copyMarkdownAction, &QAction::triggered, this, &AnnotationsPanel::copyMarkdown);

    m_deleteAction = new QAction(tr("Delete annotation"), this);
    m_deleteAction->setShortcut(QKeySequence::Delete);
    m_deleteAction->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    connect(m_deleteAction, &QAction::triggered, this, &AnnotationsPanel::deleteSelected);

    m_exportAction = new QAction(tr("Export annotations as Markdown…"), this);
    m_exportAction->setShortcut(QKeySequence(QStringLiteral("Ctrl+Shift+E")));
    connect(m_exportAction, &QAction::triggered, this, &AnnotationsPanel::exportToFile);

    m_listView->addAction(m_copyTextAction);
    m_listView->addAction(m_copyMarkdownAction);
    m_listView->addAction(m_deleteAction);

    updateActionState();
}

void AnnotationsPanel::updateActionState()
{
    const bool hasSelection = selectedIndex().isValid();
    const bool hasDocument = m_core && m_core->hasDocument();
    const bool hasRows = m_model && m_model->rowCount() > 0;
    const bool canMutate = m_core && m_core->hasPersistence();

    m_revealAction->setEnabled(hasSelection);
    m_editAction->setEnabled(hasSelection && canMutate);
    m_copyTextAction->setEnabled(hasSelection);
    m_copyMarkdownAction->setEnabled(hasSelection);
    m_deleteAction->setEnabled(hasSelection && canMutate);
    m_exportAction->setEnabled(hasDocument && hasRows);
}

QModelIndex AnnotationsPanel::selectedIndex() const
{
    if (!m_listView || !m_listView->selectionModel())
        return {};
    return m_listView->selectionModel()->currentIndex();
}

std::optional<qint64> AnnotationsPanel::selectedId() const
{
    const QModelIndex idx = selectedIndex();
    if (!idx.isValid())
        return std::nullopt;
    bool ok = false;
    const qint64 id = idx.data(TextAnnotationListModel::IdRole).toLongLong(&ok);
    if (!ok)
        return std::nullopt;
    return id;
}

void AnnotationsPanel::selectRow(int row)
{
    if (row < 0 || row >= m_model->rowCount())
        return;
    const QModelIndex idx = m_model->index(row, 0);
    m_listView->setCurrentIndex(idx);
    m_listView->selectionModel()->select(
        idx, QItemSelectionModel::ClearAndSelect | QItemSelectionModel::Rows);
    m_listView->scrollTo(idx, QAbstractItemView::EnsureVisible);
}

void AnnotationsPanel::selectRowById(qint64 id)
{
    if (const int row = m_model->rowForId(id); row >= 0)
        selectRow(row);
}

void AnnotationsPanel::focusList()
{
    if (!m_core || !m_core->hasDocument()) {
        setFocus(Qt::OtherFocusReason);
        return;
    }
    if (m_model->rowCount() == 0 && !(m_editor && m_editor->isCreating())) {
        setFocus(Qt::OtherFocusReason);
        return;
    }
    if (!selectedIndex().isValid() && m_model->rowCount() > 0)
        selectRow(0);
    m_listView->setFocus(Qt::OtherFocusReason);
}

void AnnotationsPanel::focusEditor()
{
    if (m_editor)
        m_editor->focusEditor();
}

bool AnnotationsPanel::isCreating() const
{
    return m_editor && m_editor->isCreating();
}

void AnnotationsPanel::beginCreation()
{
    if (!m_core)
        return;

    if (!m_core->hasPersistence()) {
        if (m_editor) {
            m_editor->showUnavailableMessage(
                tr("Annotation storage is not available; changes cannot be saved."));
        }
        updateEmptyState();
        return;
    }

    if (!confirmDiscardDirty(tr("create a new annotation")))
        return;

    const QString source = m_core->pendingAnnotationText();
    if (source.isEmpty()) {
        m_core->cancelPendingAnnotation();
        updateEmptyState();
        if (m_editor)
            m_editor->showUnavailableMessage(
                tr("Select text on the page before creating an annotation."));
        return;
    }

    updateEmptyState();
    if (m_stack->currentIndex() != ListPage)
        m_stack->setCurrentIndex(ListPage);
    m_editor->beginCreate(source);
    focusEditor();
}

bool AnnotationsPanel::isDirty() const
{
    return m_editor && m_editor->isDirty();
}

bool AnnotationsPanel::trySaveDirtyEditor()
{
    if (!m_core || !m_editor || !isDirty())
        return true;

    const QString markdown = m_editor->markdown();
    if (markdown.trimmed().isEmpty()) {
        QMessageBox::warning(
            this,
            tr("Save annotation"),
            tr("The annotation body cannot be empty."));
        return false;
    }

    if (m_editor->isCreating()) {
        qint64 createdId = 0;
        if (!m_core->createTextAnnotation(markdown, &createdId) || createdId == 0) {
            QMessageBox::warning(
                this,
                tr("Save annotation"),
                m_core->statusText().isEmpty()
                    ? tr("The annotation could not be created.")
                    : m_core->statusText());
            return false;
        }
        m_selectAfterRefresh = createdId;
        refresh(false);
        if (m_model->rowForId(createdId) < 0) {
            m_core->cancelPendingAnnotation();
            m_editor->clearAnnotation();
            refresh(true);
            QMessageBox::warning(
                this,
                tr("Save annotation"),
                tr("The annotation was saved but could not be selected. The list was refreshed."));
            return false;
        }
        selectRowById(createdId);
        m_editor->markSaved(createdId, markdown);
        focusList();
        return true;
    }

    const qint64 id = m_editor->annotationId();
    if (!m_core->setTextAnnotationBody(id, markdown)) {
        QMessageBox::warning(
            this,
            tr("Save annotation"),
            m_core->statusText().isEmpty()
                ? tr("The annotation could not be saved.")
                : m_core->statusText());
        return false;
    }
    refresh(false);
    m_editor->markSaved(id, markdown);
    return true;
}

bool AnnotationsPanel::confirmDiscardDirty(const QString &actionLabel)
{
    if (!isDirty())
        return true;

    QMessageBox box(this);
    box.setIcon(QMessageBox::Warning);
    box.setWindowTitle(tr("Unsaved changes"));
    box.setText(tr("You have unsaved annotation changes. Save before you %1?")
                    .arg(actionLabel));
    QPushButton *saveBtn = box.addButton(tr("Save"), QMessageBox::AcceptRole);
    QPushButton *discardBtn = box.addButton(tr("Discard"), QMessageBox::DestructiveRole);
    box.addButton(QMessageBox::Cancel);
    box.setDefaultButton(saveBtn);
    box.exec();

    if (box.clickedButton() == saveBtn)
        return trySaveDirtyEditor();

    if (box.clickedButton() != discardBtn)
        return false;

    if (m_editor && m_editor->isCreating()) {
        m_core->cancelPendingAnnotation();
        m_editor->clearAnnotation();
        updateEmptyState();
    } else if (m_editor) {
        m_editor->revert();
    }
    return true;
}

void AnnotationsPanel::clearPendingKey()
{
    clearSidebarPendingKey(&m_pendingKey, &m_pendingKeyTimer);
}

bool AnnotationsPanel::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == m_listView && event->type() == QEvent::KeyPress) {
        if (handleListKey(static_cast<QKeyEvent *>(event)))
            return true;
    }
    if (watched == m_listView && event->type() == QEvent::FocusOut)
        clearPendingKey();
    return QWidget::eventFilter(watched, event);
}

bool AnnotationsPanel::handleListKey(QKeyEvent *event)
{
    const int rows = m_model ? m_model->rowCount() : 0;
    const QModelIndex current = selectedIndex();
    const int row = current.isValid() ? current.row() : -1;

    SidebarListKeyActions actions;
    actions.selectRow = [this](int r) { selectRow(r); };
    actions.deleteSelected = [this]() { deleteSelected(); };
    actions.copySource = [this]() { copySource(); };
    actions.copyMarkdown = [this]() { copyMarkdown(); };
    actions.reveal = [this]() { revealCurrent(); };
    actions.escapeToCanvas = [this]() { emit focusCanvasRequested(); };
    actions.edit = [this]() { editCurrent(); };

    return handleSidebarListKey(
        event, rows, row, &m_pendingKey, &m_pendingKeyTimer, actions);
}

void AnnotationsPanel::onSelectionChanged()
{
    if (m_loadingSelection)
        return;

    const qint64 editorId =
        (m_editor && m_editor->hasAnnotation() && !m_editor->isCreating())
        ? m_editor->annotationId()
        : 0;

    const QModelIndex idx = selectedIndex();
    bool ok = false;
    const qint64 newId =
        idx.isValid() ? idx.data(TextAnnotationListModel::IdRole).toLongLong(&ok) : 0;

    if (m_editor && m_editor->isDirty()) {
        if (m_editor->isCreating() || !ok || newId != editorId) {
            if (!confirmDiscardDirty(tr("select another annotation"))) {
                m_loadingSelection = true;
                if (m_editor->isCreating()) {
                    m_listView->clearSelection();
                    m_listView->setCurrentIndex(QModelIndex());
                } else if (editorId != 0) {
                    selectRowById(editorId);
                } else {
                    m_listView->clearSelection();
                    m_listView->setCurrentIndex(QModelIndex());
                }
                m_loadingSelection = false;
                return;
            }
        } else {
            updateActionState();
            return;
        }
    }

    loadSelectedIntoEditor();
    updateActionState();
}

void AnnotationsPanel::loadSelectedIntoEditor()
{
    if (!m_editor || !m_model)
        return;
    if (m_editor->isCreating())
        return;

    const QModelIndex idx = selectedIndex();
    if (!idx.isValid()) {
        m_editor->clearAnnotation();
        return;
    }

    const std::optional<qint64> id = selectedId();
    if (!id)
        return;

    if (m_editor->hasAnnotation() && m_editor->annotationId() == *id)
        return;

    m_editor->loadAnnotation(m_model->itemAt(idx.row()));
}

void AnnotationsPanel::onActivated(const QModelIndex &index)
{
    if (!index.isValid() || !m_core)
        return;
    bool ok = false;
    const qint64 id = index.data(TextAnnotationListModel::IdRole).toLongLong(&ok);
    if (!ok)
        return;
    m_core->revealTextAnnotation(id);
    m_listView->setFocus(Qt::OtherFocusReason);
}

void AnnotationsPanel::revealCurrent()
{
    onActivated(selectedIndex());
}

void AnnotationsPanel::editCurrent()
{
    if (!m_core || !m_core->hasPersistence())
        return;
    const QModelIndex idx = selectedIndex();
    if (!idx.isValid() || !m_editor)
        return;
    if (m_editor->isCreating())
        return;
    if (!m_editor->hasAnnotation() || m_editor->annotationId() != selectedId()) {
        if (!confirmDiscardDirty(tr("edit this annotation")))
            return;
        loadSelectedIntoEditor();
    }
    focusEditor();
}

void AnnotationsPanel::copySource()
{
    const QModelIndex idx = selectedIndex();
    if (!idx.isValid())
        return;
    QGuiApplication::clipboard()->setText(
        idx.data(TextAnnotationListModel::TextRole).toString());
}

void AnnotationsPanel::copyMarkdown()
{
    const std::optional<qint64> id = selectedId();
    if (!id || !m_core)
        return;
    const QString markdown = m_core->textAnnotationMarkdown(*id);
    if (markdown.isEmpty())
        return;
    QGuiApplication::clipboard()->setText(markdown);
}

void AnnotationsPanel::onSaveRequested(qint64 annotationId, const QString &markdown)
{
    if (!m_core || !m_editor)
        return;

    if (markdown.trimmed().isEmpty()) {
        QMessageBox::warning(
            this,
            tr("Save annotation"),
            tr("The annotation body cannot be empty."));
        return;
    }

    if (annotationId == 0) {
        qint64 createdId = 0;
        if (!m_core->createTextAnnotation(markdown, &createdId)) {
            QMessageBox::warning(
                this,
                tr("Save annotation"),
                m_core->statusText().isEmpty()
                    ? tr("The annotation could not be created.")
                    : m_core->statusText());
            return;
        }
        if (createdId == 0) {
            m_core->cancelPendingAnnotation();
            m_editor->clearAnnotation();
            refresh(true);
            QMessageBox::warning(
                this,
                tr("Save annotation"),
                tr("The annotation was saved but could not be selected. The list was refreshed."));
            return;
        }
        m_selectAfterRefresh = createdId;
        refresh(false);
        if (m_model->rowForId(createdId) < 0) {
            m_editor->clearAnnotation();
            refresh(true);
            QMessageBox::warning(
                this,
                tr("Save annotation"),
                tr("The annotation was saved but could not be selected. The list was refreshed."));
            return;
        }
        selectRowById(createdId);
        m_editor->markSaved(createdId, markdown);
        focusList();
    } else {
        if (!m_core->setTextAnnotationBody(annotationId, markdown)) {
            QMessageBox::warning(
                this,
                tr("Save annotation"),
                m_core->statusText().isEmpty()
                    ? tr("The annotation could not be saved.")
                    : m_core->statusText());
            return;
        }
        refresh(false);
        m_editor->markSaved(annotationId, markdown);
        focusList();
    }

    updateEmptyState();
    updateActionState();
}

void AnnotationsPanel::onCancelCreate()
{
    if (m_core)
        m_core->cancelPendingAnnotation();
    if (m_editor)
        m_editor->clearAnnotation();
    updateEmptyState();
    loadSelectedIntoEditor();
    focusList();
}

void AnnotationsPanel::onEditorEscapeToList()
{
    focusList();
}

void AnnotationsPanel::deleteSelected()
{
    if (!m_core || !m_core->hasPersistence())
        return;

    if (!confirmDiscardDirty(tr("delete this annotation")))
        return;

    const QModelIndex idx = selectedIndex();
    const std::optional<qint64> id = selectedId();
    if (!idx.isValid() || !id)
        return;

    const QString pageLabel = pageLabelForIndex(idx);

    QMessageBox box(this);
    box.setIcon(QMessageBox::Warning);
    box.setWindowTitle(tr("Delete annotation"));
    box.setText(tr("Delete the annotation on %1?").arg(pageLabel));
    QPushButton *deleteBtn = box.addButton(tr("Delete"), QMessageBox::DestructiveRole);
    box.addButton(QMessageBox::Cancel);
    box.setDefaultButton(QMessageBox::Cancel);
    box.exec();
    if (box.clickedButton() != deleteBtn)
        return;

    m_rowAfterDelete = idx.row();
    if (!m_core->deleteTextAnnotation(*id)) {
        m_rowAfterDelete = -1;
        QMessageBox::warning(
            this,
            tr("Delete annotation"),
            m_core->statusText().isEmpty()
                ? tr("The annotation could not be deleted.")
                : m_core->statusText());
        return;
    }

    if (m_editor)
        m_editor->clearAnnotation();

    if (m_model->rowCount() == 0)
        emit focusCanvasRequested();
    else
        m_listView->setFocus(Qt::OtherFocusReason);
}

void AnnotationsPanel::exportToFile()
{
    if (!m_core)
        return;
    const QString markdown = m_core->allTextAnnotationsMarkdown();
    if (markdown.isEmpty()) {
        QMessageBox::information(
            this,
            tr("Export annotations"),
            tr("This document has no annotations to export."));
        return;
    }

    const QString documentPath = m_core->documentPath();
    const QString base = documentPath.isEmpty()
        ? QStringLiteral("annotations")
        : QFileInfo(documentPath).completeBaseName() + QStringLiteral("-annotations");
    const QString suggestion = documentPath.isEmpty()
        ? base + QStringLiteral(".md")
        : QFileInfo(documentPath).dir().filePath(base + QStringLiteral(".md"));

    const QString path = QFileDialog::getSaveFileName(
        this,
        tr("Export annotations as Markdown"),
        suggestion,
        tr("Markdown (*.md);;All files (*)"));
    if (path.isEmpty())
        return;

    QString error;
    if (!writeTextAnnotationsMarkdown(path, markdown, &error)) {
        QMessageBox::warning(
            this,
            tr("Export annotations"),
            tr("Cannot write %1: %2").arg(path, error));
        return;
    }
    showExportStatus(this, path);
}

} // namespace syodep
