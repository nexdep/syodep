#include "sidebar/highlights_panel.h"

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
#include <QMenu>
#include <QMessageBox>
#include <QPushButton>
#include <QSaveFile>
#include <QSet>
#include <QShowEvent>
#include <QStackedWidget>
#include <QVBoxLayout>

#include "core_controller.h"
#include "sidebar/highlight_delegate.h"
#include "sidebar/highlight_list_model.h"
#include "sidebar/sidebar_list_keys.h"

#include <QMainWindow>
#include <QStatusBar>

namespace syodep {

namespace {

// Fallback for the two-key sequences when the core cannot say what the
// configured timeout is (no core, or a core built without one). Long enough to
// type `dd` deliberately, short enough that a stray `d` does not stay armed.
constexpr int kFallbackSequenceTimeoutMs = 600;

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

bool writeHighlightsMarkdown(const QString &path, const QString &markdown, QString *error)
{
    QSaveFile file(path);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Text)) {
        if (error)
            *error = describeSaveFileError(file, QObject::tr("cannot open for writing"));
        return false;
    }
    QByteArray payload = markdown.toUtf8();
    // A text file ends with a newline; the core's Markdown does not, because
    // it is also used for the clipboard.
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

HighlightsPanel::HighlightsPanel(CoreController *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
{
    Q_ASSERT(m_core);
    // Empty / no-document pages have no child that takes keys; StrongFocus lets
    // `<leader>a` park the keyboard on the sidebar itself until there is a list.
    setFocusPolicy(Qt::StrongFocus);

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

    m_listView = new QListView(m_stack);
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
    m_listView->installEventFilter(this);
    m_stack->addWidget(m_listView);

    m_pendingKeyTimer.setSingleShot(true);
    const int timeout = m_core->keyTimeoutMs();
    m_pendingKeyTimer.setInterval(timeout > 0 ? timeout : kFallbackSequenceTimeoutMs);
    connect(&m_pendingKeyTimer, &QTimer::timeout, this, &HighlightsPanel::clearPendingKey);

    setupActions();

    connect(m_core, &CoreController::documentChanged,
            this, &HighlightsPanel::onDocumentChanged);
    connect(m_core, &CoreController::annotationsChanged,
            this, &HighlightsPanel::onAnnotationsChanged);

    connect(m_listView, &QListView::activated,
            this, &HighlightsPanel::onActivated);
    connect(m_listView, &QListView::customContextMenuRequested,
            this, &HighlightsPanel::onCustomContextMenu);
    connect(m_listView->selectionModel(), &QItemSelectionModel::currentChanged,
            this, [this](const QModelIndex &, const QModelIndex &) { updateActionState(); });

    refreshAnnotations(true);
}

void HighlightsPanel::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    if (!m_initialRefreshDone) {
        m_initialRefreshDone = true;
        refreshAnnotations(true);
    }
}

void HighlightsPanel::onDocumentChanged()
{
    m_rowAfterDelete = -1;
    clearPendingKey();
    refreshAnnotations(true);
}

void HighlightsPanel::onAnnotationsChanged()
{
    refreshAnnotations(false);
}

void HighlightsPanel::refreshAnnotations(bool force)
{
    if (!m_core)
        return;

    const quint64 revision = m_core->annotationRevision();
    if (!force && revision == m_model->revision()) {
        updateEmptyState();
        return;
    }

    const std::optional<qint64> previousId = selectedId();
    QSet<qint64> previousIds;
    for (int i = 0; i < m_model->rowCount(); ++i) {
        if (const HighlightListItem *item = m_model->itemAt(i))
            previousIds.insert(item->id);
    }

    HighlightSnapshot snapshot;
    if (m_core->hasDocument())
        snapshot = m_core->highlightSnapshot();

    m_model->setSnapshot(std::move(snapshot));

    // A single newly appeared id is a commit: select it wherever document order
    // put it. Anything else (open, import, multi-change) restores by id, or by
    // the row a delete left behind.
    QVector<qint64> added;
    for (int i = 0; i < m_model->rowCount(); ++i) {
        if (const HighlightListItem *item = m_model->itemAt(i);
            item && !previousIds.contains(item->id)) {
            added.push_back(item->id);
        }
    }

    if (added.size() == 1) {
        selectRowById(added.first());
    } else if (previousId && m_model->rowForId(*previousId)) {
        selectRowById(*previousId);
    } else if (m_rowAfterDelete >= 0 && m_model->rowCount() > 0) {
        selectRow(qMin(m_rowAfterDelete, m_model->rowCount() - 1));
    } else {
        m_listView->clearSelection();
        m_listView->setCurrentIndex(QModelIndex());
    }
    m_rowAfterDelete = -1;

    updateEmptyState();
    updateActionState();
}

void HighlightsPanel::updateEmptyState()
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

HighlightsPanel::ContentState HighlightsPanel::contentState() const
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

void HighlightsPanel::setupActions()
{
    m_revealAction = new QAction(tr("Go to highlight"), this);
    connect(m_revealAction, &QAction::triggered, this, &HighlightsPanel::revealCurrent);

    m_copyTextAction = new QAction(tr("Copy highlighted text"), this);
    m_copyTextAction->setShortcut(QKeySequence::Copy);
    m_copyTextAction->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    addAction(m_copyTextAction);
    connect(m_copyTextAction, &QAction::triggered, this, &HighlightsPanel::copyHighlightedText);

    m_copyMarkdownAction = new QAction(tr("Copy as Markdown"), this);
    m_copyMarkdownAction->setShortcut(
        QKeySequence(QStringLiteral("Ctrl+Shift+C")));
    m_copyMarkdownAction->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    addAction(m_copyMarkdownAction);
    connect(m_copyMarkdownAction, &QAction::triggered, this, &HighlightsPanel::copyAsMarkdown);

    m_copyAllAction = new QAction(tr("Copy all highlights as Markdown"), this);
    connect(m_copyAllAction, &QAction::triggered, this, &HighlightsPanel::copyAllAsMarkdown);

    // `Delete` belongs to the action so the menu can advertise it; `dd` is
    // handled in the event filter. One of the two must own the key, or the
    // shortcut would swallow the event before the filter ever saw it.
    m_deleteAction = new QAction(tr("Delete highlight"), this);
    m_deleteAction->setShortcut(QKeySequence::Delete);
    m_deleteAction->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    connect(m_deleteAction, &QAction::triggered,
            this, &HighlightsPanel::deleteSelectedHighlight);

    // Window-scoped: the window puts this in its menu, which is what keeps the
    // shortcut working while the dock is closed.
    m_exportAction = new QAction(tr("Export highlights as Markdown…"), this);
    m_exportAction->setShortcut(QKeySequence(QStringLiteral("Ctrl+Shift+E")));
    connect(m_exportAction, &QAction::triggered,
            this, &HighlightsPanel::exportHighlightsToFile);

    m_listView->addAction(m_copyTextAction);
    m_listView->addAction(m_copyMarkdownAction);
    m_listView->addAction(m_deleteAction);

    updateActionState();
}

void HighlightsPanel::updateActionState()
{
    const bool hasSelection = selectedIndex().isValid();
    const bool hasDocument = m_core && m_core->hasDocument();
    const bool hasRows = m_model && m_model->rowCount() > 0;

    m_revealAction->setEnabled(hasSelection);
    m_copyTextAction->setEnabled(hasSelection);
    m_copyMarkdownAction->setEnabled(hasSelection);
    m_deleteAction->setEnabled(hasSelection);
    m_copyAllAction->setEnabled(hasDocument && hasRows);
    m_exportAction->setEnabled(hasDocument && hasRows);
}

QModelIndex HighlightsPanel::selectedIndex() const
{
    if (!m_listView || !m_listView->selectionModel())
        return {};
    return m_listView->selectionModel()->currentIndex();
}

std::optional<qint64> HighlightsPanel::selectedId() const
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

void HighlightsPanel::selectRow(int row)
{
    if (row < 0 || row >= m_model->rowCount())
        return;
    const QModelIndex idx = m_model->index(row, 0);
    m_listView->setCurrentIndex(idx);
    m_listView->selectionModel()->select(
        idx, QItemSelectionModel::ClearAndSelect | QItemSelectionModel::Rows);
    m_listView->scrollTo(idx, QAbstractItemView::EnsureVisible);
}

void HighlightsPanel::selectRowById(qint64 id)
{
    if (const std::optional<int> row = m_model->rowForId(id))
        selectRow(*row);
}

void HighlightsPanel::focusList()
{
    // Empty / no-document pages live on the stack, not in the list view.
    // Focus the sidebar itself so `<leader>a` does not bounce the keyboard
    // straight back to the canvas when there is nothing to select.
    if (!m_listView || m_model->rowCount() == 0) {
        setFocus(Qt::OtherFocusReason);
        return;
    }
    if (!selectedIndex().isValid())
        selectRow(0);
    m_listView->setFocus(Qt::OtherFocusReason);
}

void HighlightsPanel::clearPendingKey()
{
    clearSidebarPendingKey(&m_pendingKey, &m_pendingKeyTimer);
}

bool HighlightsPanel::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == m_listView && event->type() == QEvent::KeyPress) {
        if (handleListKey(static_cast<QKeyEvent *>(event)))
            return true;
    }
    if (watched == m_listView && event->type() == QEvent::FocusOut)
        clearPendingKey();
    return QWidget::eventFilter(watched, event);
}

bool HighlightsPanel::handleListKey(QKeyEvent *event)
{
    const int rows = m_model ? m_model->rowCount() : 0;
    const QModelIndex current = selectedIndex();
    const int row = current.isValid() ? current.row() : -1;

    SidebarListKeyActions actions;
    actions.selectRow = [this](int r) { selectRow(r); };
    actions.deleteSelected = [this]() { deleteSelectedHighlight(); };
    actions.copySource = [this]() { copyHighlightedText(); };
    actions.copyMarkdown = [this]() { copyAsMarkdown(); };
    actions.reveal = [this]() { revealCurrent(); };
    actions.escapeToCanvas = [this]() { emit focusCanvasRequested(); };

    return handleSidebarListKey(
        event, rows, row, &m_pendingKey, &m_pendingKeyTimer, actions);
}

void HighlightsPanel::revealIndex(const QModelIndex &index)
{
    if (!index.isValid() || !m_core)
        return;
    bool ok = false;
    const qint64 id = index.data(HighlightListModel::IdRole).toLongLong(&ok);
    if (!ok)
        return;
    m_core->revealHighlight(id);
    // Focus stays here: jumping to a highlight is usually one of several, and
    // the canvas is one Escape away.
    m_listView->setFocus(Qt::OtherFocusReason);
}

void HighlightsPanel::onActivated(const QModelIndex &index)
{
    revealIndex(index);
}

void HighlightsPanel::revealCurrent()
{
    revealIndex(selectedIndex());
}

void HighlightsPanel::copyHighlightedText()
{
    const QModelIndex idx = selectedIndex();
    if (!idx.isValid())
        return;
    QGuiApplication::clipboard()->setText(idx.data(HighlightListModel::TextRole).toString());
}

void HighlightsPanel::copyAsMarkdown()
{
    const std::optional<qint64> id = selectedId();
    if (!id || !m_core)
        return;
    const QString markdown = m_core->highlightMarkdown(*id);
    if (markdown.isEmpty())
        return;
    QGuiApplication::clipboard()->setText(markdown);
}

void HighlightsPanel::copyAllAsMarkdown()
{
    if (!m_core)
        return;
    const QString markdown = m_core->allHighlightsMarkdown();
    if (markdown.isEmpty() && m_model->rowCount() > 0)
        return;
    QGuiApplication::clipboard()->setText(markdown);
}

void HighlightsPanel::deleteSelectedHighlight()
{
    const QModelIndex idx = selectedIndex();
    const std::optional<qint64> id = selectedId();
    if (!idx.isValid() || !id || !m_core)
        return;

    const auto state = static_cast<HighlightState>(
        idx.data(HighlightListModel::StateRole).toInt());
    const QString pageLabel = idx.data(HighlightListModel::PageLabelRole).toString();

    QMessageBox box(this);
    box.setIcon(QMessageBox::Warning);
    box.setWindowTitle(tr("Delete highlight"));
    box.setText(tr("Delete the highlight on %1?").arg(pageLabel));
    // The two cases are genuinely different promises: one touches only
    // syodep's database, the other rewrites the PDF on disk.
    box.setInformativeText(state == HighlightState::Pending
        ? tr("It has not been saved into the PDF yet, so only syodep's copy "
             "is removed.")
        : tr("It is embedded in the PDF. The annotation will be removed from "
             "the file itself, which rewrites it."));
    QPushButton *deleteBtn = box.addButton(tr("Delete"), QMessageBox::DestructiveRole);
    box.addButton(QMessageBox::Cancel);
    box.setDefaultButton(QMessageBox::Cancel);
    box.exec();
    if (box.clickedButton() != deleteBtn)
        return;

    // Remember the position, not the id: the row that moves up into this slot
    // is the one the user is now looking at.
    m_rowAfterDelete = idx.row();
    if (!m_core->deleteHighlight(*id)) {
        m_rowAfterDelete = -1;
        QMessageBox::warning(
            this,
            tr("Delete highlight"),
            m_core->statusText().isEmpty()
                ? tr("The highlight could not be deleted.")
                : m_core->statusText());
        return;
    }

    // deleteHighlight() emitted annotationsChanged, so the list has already
    // been rebuilt and the replacement row selected.
    if (m_model->rowCount() == 0)
        emit focusCanvasRequested();
    else
        m_listView->setFocus(Qt::OtherFocusReason);
}

void HighlightsPanel::exportHighlightsToFile()
{
    if (!m_core)
        return;
    const QString markdown = m_core->allHighlightsMarkdown();
    if (markdown.isEmpty()) {
        // An empty file is not a useful export; say so instead of writing one.
        QMessageBox::information(
            this,
            tr("Export highlights"),
            tr("This document has no highlights to export."));
        return;
    }

    const QString documentPath = m_core->documentPath();
    const QString base = documentPath.isEmpty()
        ? QStringLiteral("highlights")
        : QFileInfo(documentPath).completeBaseName() + QStringLiteral("-highlights");
    const QString suggestion = documentPath.isEmpty()
        ? base + QStringLiteral(".md")
        : QFileInfo(documentPath).dir().filePath(base + QStringLiteral(".md"));

    const QString path = QFileDialog::getSaveFileName(
        this,
        tr("Export highlights as Markdown"),
        suggestion,
        tr("Markdown (*.md);;All files (*)"));
    if (path.isEmpty())
        return;

    QString error;
    if (!writeHighlightsMarkdown(path, markdown, &error)) {
        QMessageBox::warning(
            this,
            tr("Export highlights"),
            tr("Cannot write %1: %2").arg(path, error));
        return;
    }
    showExportStatus(this, path);
}

void HighlightsPanel::onCustomContextMenu(const QPoint &pos)
{
    updateActionState();
    QMenu menu(this);
    menu.addAction(m_revealAction);
    menu.addSeparator();
    menu.addAction(m_copyTextAction);
    menu.addAction(m_copyMarkdownAction);
    menu.addAction(m_copyAllAction);
    menu.addAction(m_exportAction);
    menu.addSeparator();
    menu.addAction(m_deleteAction);
    menu.exec(m_listView->viewport()->mapToGlobal(pos));
}

} // namespace syodep
