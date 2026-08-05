#include "sidebar/text_annotation_editor.h"

#include <QHBoxLayout>
#include <QKeySequence>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QSignalBlocker>
#include <QStackedWidget>
#include <QTabWidget>
#include <QVBoxLayout>

#include "sidebar/safe_markdown_view.h"

namespace syodep {

namespace {

QString pageLabel(qsizetype firstPage, qsizetype lastPage)
{
    if (firstPage == lastPage)
        return QObject::tr("Page %1").arg(firstPage + 1);
    return QObject::tr("Pages %1–%2").arg(firstPage + 1).arg(lastPage + 1);
}

} // namespace

TextAnnotationEditor::TextAnnotationEditor(QWidget *parent)
    : QWidget(parent)
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(8, 8, 8, 8);
    root->setSpacing(6);

    m_stack = new QStackedWidget(this);
    root->addWidget(m_stack);

    m_emptyLabel = new QLabel(tr("Select an annotation to view or edit."), this);
    m_emptyLabel->setWordWrap(true);
    m_emptyLabel->setAlignment(Qt::AlignCenter);
    m_emptyLabel->setForegroundRole(QPalette::PlaceholderText);
    m_stack->addWidget(m_emptyLabel);

    m_editorPage = new QWidget(this);
    auto *pageLayout = new QVBoxLayout(m_editorPage);
    pageLayout->setContentsMargins(0, 0, 0, 0);
    pageLayout->setSpacing(6);

    m_pageLabel = new QLabel(m_editorPage);
    m_pageLabel->setForegroundRole(QPalette::PlaceholderText);
    pageLayout->addWidget(m_pageLabel);

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
    m_editor->setPlaceholderText(tr("Write a Markdown annotation…"));
    m_editor->setTabChangesFocus(false);
    // Placeholder until the Preview tab is opened — avoids constructing a
    // QTextBrowser beside a failed GL context during graphics probing.
    m_previewPlaceholder = new QWidget(m_tabs);
    m_tabs->addTab(m_editor, tr("Edit"));
    m_tabs->addTab(m_previewPlaceholder, tr("Preview"));
    pageLayout->addWidget(m_tabs, 1);

    m_dirtyLabel = new QLabel(m_editorPage);
    m_dirtyLabel->setForegroundRole(QPalette::Link);
    m_dirtyLabel->hide();
    pageLayout->addWidget(m_dirtyLabel);

    auto *actions = new QHBoxLayout;
    m_saveButton = new QPushButton(tr("Save"), m_editorPage);
    m_saveButton->setShortcut(QKeySequence::Save);
    m_revertButton = new QPushButton(tr("Revert"), m_editorPage);
    m_cancelButton = new QPushButton(tr("Cancel"), m_editorPage);
    actions->addWidget(m_saveButton);
    actions->addWidget(m_revertButton);
    actions->addWidget(m_cancelButton);
    actions->addStretch(1);
    pageLayout->addLayout(actions);

    m_stack->addWidget(m_editorPage);
    m_stack->setCurrentWidget(m_emptyLabel);

    connect(m_editor, &QPlainTextEdit::textChanged,
            this, &TextAnnotationEditor::onTextChanged);
    connect(m_tabs, &QTabWidget::currentChanged,
            this, &TextAnnotationEditor::onTabChanged);
    connect(m_saveButton, &QPushButton::clicked,
            this, &TextAnnotationEditor::onSaveClicked);
    connect(m_revertButton, &QPushButton::clicked,
            this, &TextAnnotationEditor::onRevertClicked);
    connect(m_cancelButton, &QPushButton::clicked,
            this, &TextAnnotationEditor::onCancelClicked);

    updateActions();
}

void TextAnnotationEditor::ensurePreview()
{
    if (m_preview)
        return;

    m_preview = new SafeMarkdownView(m_tabs);
    const int previewIndex = m_tabs->indexOf(m_previewPlaceholder);
    if (previewIndex >= 0) {
        m_tabs->removeTab(previewIndex);
        m_previewPlaceholder->deleteLater();
        m_previewPlaceholder = nullptr;
        m_tabs->insertTab(previewIndex, m_preview, tr("Preview"));
        m_tabs->setCurrentIndex(previewIndex);
    } else {
        m_tabs->addTab(m_preview, tr("Preview"));
    }
}

void TextAnnotationEditor::loadAnnotation(const TextAnnotationListItem &item)
{
    m_loading = true;
    m_creating = false;
    m_annotationId = item.id;
    m_sourceText = item.text;
    m_savedMarkdown = item.bodyMarkdown;
    {
        const QSignalBlocker blocker(m_editor);
        m_editor->setPlainText(m_savedMarkdown);
    }
    m_pageLabel->setText(pageLabel(item.firstPage, item.lastPage));
    setSourceQuote(item.text);
    m_stack->setCurrentWidget(m_editorPage);
    setDirty(false);
    if (m_tabs->currentIndex() == 1)
        updatePreview();
    updateActions();
    m_loading = false;
}

void TextAnnotationEditor::beginCreate(const QString &sourceText)
{
    m_loading = true;
    m_creating = true;
    m_annotationId = 0;
    m_sourceText = sourceText;
    m_savedMarkdown.clear();
    {
        const QSignalBlocker blocker(m_editor);
        m_editor->clear();
    }
    m_pageLabel->setText(tr("New annotation"));
    setSourceQuote(sourceText);
    m_stack->setCurrentWidget(m_editorPage);
    setDirty(false);
    if (m_tabs->currentIndex() == 1)
        updatePreview();
    updateActions();
    m_loading = false;
    focusEditor();
}

void TextAnnotationEditor::clearAnnotation()
{
    m_loading = true;
    m_creating = false;
    m_annotationId = 0;
    m_sourceText.clear();
    m_savedMarkdown.clear();
    {
        const QSignalBlocker blocker(m_editor);
        m_editor->clear();
    }
    m_pageLabel->clear();
    m_sourceQuote->clear();
    if (m_preview)
        m_preview->setMarkdown(QString());
    m_emptyLabel->setText(tr("Select an annotation to view or edit."));
    m_stack->setCurrentWidget(m_emptyLabel);
    setDirty(false);
    updateActions();
    m_loading = false;
}

void TextAnnotationEditor::showUnavailableMessage(const QString &message)
{
    clearAnnotation();
    m_emptyLabel->setText(message);
    m_stack->setCurrentWidget(m_emptyLabel);
}

bool TextAnnotationEditor::isDirty() const
{
    return m_dirty;
}

QString TextAnnotationEditor::markdown() const
{
    return m_editor->toPlainText();
}

void TextAnnotationEditor::markSaved(qint64 annotationId, const QString &savedBody)
{
    m_creating = false;
    m_annotationId = annotationId;
    m_savedMarkdown = savedBody;
    setDirty(m_editor->toPlainText() != m_savedMarkdown);
    updateActions();
}

void TextAnnotationEditor::reloadPreservingDraft(const TextAnnotationListItem &item,
                                                   const QString &savedBaseline,
                                                   const QString &draft)
{
    m_loading = true;
    m_creating = false;
    m_annotationId = item.id;
    m_sourceText = item.text;
    m_savedMarkdown = savedBaseline;
    {
        const QSignalBlocker blocker(m_editor);
        m_editor->setPlainText(draft);
    }
    m_pageLabel->setText(pageLabel(item.firstPage, item.lastPage));
    setSourceQuote(item.text);
    m_stack->setCurrentWidget(m_editorPage);
    setDirty(draft != savedBaseline);
    if (m_tabs->currentIndex() == 1)
        updatePreview();
    updateActions();
    m_loading = false;
}

void TextAnnotationEditor::revert()
{
    if (m_creating) {
        const QSignalBlocker blocker(m_editor);
        m_editor->clear();
        setDirty(false);
        if (m_tabs->currentIndex() == 1)
            updatePreview();
        updateActions();
        return;
    }
    const QSignalBlocker blocker(m_editor);
    m_editor->setPlainText(m_savedMarkdown);
    setDirty(false);
    if (m_tabs->currentIndex() == 1)
        updatePreview();
    updateActions();
}

void TextAnnotationEditor::focusEditor()
{
    m_tabs->setCurrentIndex(0);
    m_editor->setFocus(Qt::OtherFocusReason);
}

void TextAnnotationEditor::onTextChanged()
{
    if (m_loading)
        return;
    if (m_creating)
        setDirty(!m_editor->toPlainText().isEmpty());
    else
        setDirty(m_editor->toPlainText() != m_savedMarkdown);
    updateActions();
}

void TextAnnotationEditor::onTabChanged(int index)
{
    if (index == 1)
        updatePreview();
}

void TextAnnotationEditor::onSaveClicked()
{
    if (!hasAnnotation())
        return;
    emit saveRequested(m_annotationId, m_editor->toPlainText());
}

void TextAnnotationEditor::onRevertClicked()
{
    revert();
}

void TextAnnotationEditor::onCancelClicked()
{
    if (m_creating)
        emit cancelCreateRequested();
    else
        revert();
}

void TextAnnotationEditor::updatePreview()
{
    ensurePreview();
    if (m_preview)
        m_preview->setMarkdown(m_editor->toPlainText());
}

void TextAnnotationEditor::updateActions()
{
    const bool enabled = hasAnnotation();
    m_editor->setEnabled(enabled);
    m_saveButton->setEnabled(enabled && (m_creating || m_dirty));
    m_revertButton->setEnabled(enabled && m_dirty);
    m_cancelButton->setVisible(m_creating);
    m_cancelButton->setEnabled(m_creating);
    m_dirtyLabel->setVisible(enabled && m_dirty);
    m_dirtyLabel->setText(m_dirty ? tr("Unsaved changes") : QString());
}

void TextAnnotationEditor::setDirty(bool dirty)
{
    if (m_dirty == dirty)
        return;
    m_dirty = dirty;
    emit dirtyStateChanged(m_dirty);
}

void TextAnnotationEditor::setSourceQuote(const QString &text)
{
    QString quote = text;
    if (quote.size() > 280)
        quote = quote.left(277) + QStringLiteral("…");
    m_sourceQuote->setText(quote.isEmpty() ? tr("(empty source quote)") : quote);
}

} // namespace syodep
