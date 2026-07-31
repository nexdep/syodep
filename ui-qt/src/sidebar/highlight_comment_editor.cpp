#include "sidebar/highlight_comment_editor.h"

#include <QHBoxLayout>
#include <QKeySequence>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QSignalBlocker>
#include <QStackedWidget>
#include <QTabWidget>
#include <QVBoxLayout>

#include "sidebar/highlight_list_model.h"
#include "sidebar/safe_markdown_view.h"

namespace syodep {

HighlightCommentEditor::HighlightCommentEditor(QWidget *parent)
    : QWidget(parent)
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(8, 8, 8, 8);
    root->setSpacing(6);

    m_stack = new QStackedWidget(this);
    root->addWidget(m_stack);

    m_emptyLabel = new QLabel(tr("Select a highlight to view or add a comment."), this);
    m_emptyLabel->setWordWrap(true);
    m_emptyLabel->setAlignment(Qt::AlignCenter);
    m_emptyLabel->setForegroundRole(QPalette::PlaceholderText);
    m_stack->addWidget(m_emptyLabel);

    m_editorPage = new QWidget(this);
    auto *pageLayout = new QVBoxLayout(m_editorPage);
    pageLayout->setContentsMargins(0, 0, 0, 0);
    pageLayout->setSpacing(6);

    auto *metaRow = new QHBoxLayout;
    m_pageLabel = new QLabel(m_editorPage);
    m_pageLabel->setForegroundRole(QPalette::PlaceholderText);
    m_stateLabel = new QLabel(m_editorPage);
    m_stateLabel->setForegroundRole(QPalette::PlaceholderText);
    m_stateLabel->setAlignment(Qt::AlignRight | Qt::AlignVCenter);
    metaRow->addWidget(m_pageLabel);
    metaRow->addStretch(1);
    metaRow->addWidget(m_stateLabel);
    pageLayout->addLayout(metaRow);

    auto *quoteLabel = new QLabel(tr("Source"), m_editorPage);
    QFont quoteHeading = quoteLabel->font();
    quoteHeading.setBold(true);
    quoteHeading.setPointSizeF(qMax(8.0, quoteHeading.pointSizeF() - 1.0));
    quoteLabel->setFont(quoteHeading);
    quoteLabel->setForegroundRole(QPalette::PlaceholderText);
    pageLayout->addWidget(quoteLabel);

    m_sourceQuote = new QLabel(m_editorPage);
    m_sourceQuote->setWordWrap(true);
    m_sourceQuote->setTextInteractionFlags(Qt::TextSelectableByMouse);
    m_sourceQuote->setForegroundRole(QPalette::WindowText);
    m_sourceQuote->setMaximumHeight(QFontMetrics(font()).lineSpacing() * 4);
    pageLayout->addWidget(m_sourceQuote);

    m_tabs = new QTabWidget(m_editorPage);
    m_editor = new QPlainTextEdit(m_tabs);
    m_editor->setPlaceholderText(tr("Add a Markdown comment…"));
    m_editor->setTabChangesFocus(false);
    m_preview = new SafeMarkdownView(m_tabs);
    m_tabs->addTab(m_editor, tr("Edit"));
    m_tabs->addTab(m_preview, tr("Preview"));
    pageLayout->addWidget(m_tabs, 1);

    m_dirtyLabel = new QLabel(m_editorPage);
    m_dirtyLabel->setForegroundRole(QPalette::Link);
    m_dirtyLabel->hide();
    pageLayout->addWidget(m_dirtyLabel);

    auto *actions = new QHBoxLayout;
    m_saveButton = new QPushButton(tr("Save"), m_editorPage);
    m_saveButton->setShortcut(QKeySequence::Save);
    m_revertButton = new QPushButton(tr("Revert"), m_editorPage);
    actions->addWidget(m_saveButton);
    actions->addWidget(m_revertButton);
    actions->addStretch(1);
    pageLayout->addLayout(actions);

    m_stack->addWidget(m_editorPage);
    m_stack->setCurrentWidget(m_emptyLabel);

    connect(m_editor, &QPlainTextEdit::textChanged, this, &HighlightCommentEditor::onTextChanged);
    connect(m_tabs, &QTabWidget::currentChanged, this, &HighlightCommentEditor::onTabChanged);
    connect(m_saveButton, &QPushButton::clicked, this, &HighlightCommentEditor::onSaveClicked);
    connect(m_revertButton, &QPushButton::clicked, this, &HighlightCommentEditor::onRevertClicked);

    updateActions();
}

void HighlightCommentEditor::loadHighlight(const HighlightListItem &item)
{
    m_loading = true;
    m_highlightId = item.id;
    m_savedMarkdown = item.hasNote ? item.noteMarkdown : QString();
    {
        const QSignalBlocker blocker(m_editor);
        m_editor->setPlainText(m_savedMarkdown);
    }
    m_pageLabel->setText(highlightPageLabel(item.firstPage, item.lastPage));
    m_stateLabel->setText(highlightStateLabel(item.state));
    QString quote = item.text.trimmed();
    if (quote.size() > 280)
        quote = quote.left(277) + QStringLiteral("…");
    m_sourceQuote->setText(quote.isEmpty() ? tr("(empty source quote)") : quote);
    m_stack->setCurrentWidget(m_editorPage);
    setDirty(false);
    updatePreview();
    updateActions();
    m_loading = false;
}

void HighlightCommentEditor::clearHighlight()
{
    m_loading = true;
    m_highlightId = 0;
    m_savedMarkdown.clear();
    {
        const QSignalBlocker blocker(m_editor);
        m_editor->clear();
    }
    m_pageLabel->clear();
    m_stateLabel->clear();
    m_sourceQuote->clear();
    m_preview->setMarkdown(QString());
    m_emptyLabel->setText(tr("Select a highlight to view or add a comment."));
    m_stack->setCurrentWidget(m_emptyLabel);
    setDirty(false);
    updateActions();
    m_loading = false;
}

void HighlightCommentEditor::showUnavailableMessage(const QString &message)
{
    clearHighlight();
    m_emptyLabel->setText(message);
    m_stack->setCurrentWidget(m_emptyLabel);
}

bool HighlightCommentEditor::isDirty() const
{
    return m_dirty;
}

QString HighlightCommentEditor::markdown() const
{
    return m_editor->toPlainText();
}

void HighlightCommentEditor::markSaved(const QString &savedBody)
{
    m_savedMarkdown = savedBody;
    setDirty(m_editor->toPlainText() != m_savedMarkdown);
    updateActions();
}

void HighlightCommentEditor::reloadPreservingDraft(const HighlightListItem &item,
                                                   const QString &savedBaseline,
                                                   const QString &draft)
{
    m_loading = true;
    m_highlightId = item.id;
    m_savedMarkdown = savedBaseline;
    {
        const QSignalBlocker blocker(m_editor);
        m_editor->setPlainText(draft);
    }
    m_pageLabel->setText(highlightPageLabel(item.firstPage, item.lastPage));
    m_stateLabel->setText(highlightStateLabel(item.state));
    QString quote = item.text.trimmed();
    if (quote.size() > 280)
        quote = quote.left(277) + QStringLiteral("…");
    m_sourceQuote->setText(quote.isEmpty() ? tr("(empty source quote)") : quote);
    m_stack->setCurrentWidget(m_editorPage);
    setDirty(draft != savedBaseline);
    updatePreview();
    updateActions();
    m_loading = false;
}

void HighlightCommentEditor::revert()
{
    const QSignalBlocker blocker(m_editor);
    m_editor->setPlainText(m_savedMarkdown);
    setDirty(false);
    updatePreview();
    updateActions();
}

void HighlightCommentEditor::onTextChanged()
{
    if (m_loading)
        return;
    setDirty(m_editor->toPlainText() != m_savedMarkdown);
    updateActions();
}

void HighlightCommentEditor::onTabChanged(int index)
{
    if (index == 1)
        updatePreview();
}

void HighlightCommentEditor::onSaveClicked()
{
    if (!hasHighlight())
        return;
    emit saveRequested(m_highlightId, m_editor->toPlainText());
}

void HighlightCommentEditor::onRevertClicked()
{
    revert();
}

void HighlightCommentEditor::updatePreview()
{
    m_preview->setMarkdown(m_editor->toPlainText());
}

void HighlightCommentEditor::updateActions()
{
    const bool enabled = hasHighlight();
    m_editor->setEnabled(enabled);
    m_saveButton->setEnabled(enabled && m_dirty);
    m_revertButton->setEnabled(enabled && m_dirty);
    m_dirtyLabel->setVisible(enabled && m_dirty);
    m_dirtyLabel->setText(m_dirty ? tr("Unsaved changes") : QString());
}

void HighlightCommentEditor::setDirty(bool dirty)
{
    if (m_dirty == dirty)
        return;
    m_dirty = dirty;
    emit dirtyStateChanged(m_dirty);
}

} // namespace syodep
