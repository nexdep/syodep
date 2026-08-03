// Markdown editor for an independent text annotation (create or edit).
#pragma once

#include <QWidget>

#include "core_controller.h"

class QLabel;
class QPlainTextEdit;
class QPushButton;
class QTabWidget;
class QStackedWidget;
class QEvent;

namespace syodep {

class SafeMarkdownView;

class TextAnnotationEditor final : public QWidget
{
    Q_OBJECT

public:
    explicit TextAnnotationEditor(QWidget *parent = nullptr);

    // Edit an existing annotation (id != 0).
    void loadAnnotation(const TextAnnotationListItem &item);
    // Create mode: show captured source; id is 0 until first successful save.
    void beginCreate(const QString &sourceText);
    void clearAnnotation();
    void showUnavailableMessage(const QString &message);

    bool isDirty() const;
    bool isCreating() const { return m_creating; }
    bool hasAnnotation() const { return m_annotationId != 0 || m_creating; }
    qint64 annotationId() const { return m_annotationId; }
    QString markdown() const;
    QString savedMarkdown() const { return m_savedMarkdown; }
    QString sourceText() const { return m_sourceText; }

    void markSaved(qint64 annotationId, const QString &savedBody);
    void revert();
    void reloadPreservingDraft(const TextAnnotationListItem &item,
                               const QString &savedBaseline,
                               const QString &draft);

    void focusEditor();

signals:
    // annotationId == 0 means create from pending anchor.
    void saveRequested(qint64 annotationId, const QString &markdown);
    void cancelCreateRequested();
    // Clean Escape: leave the editor and focus the list. Dirty Escape stays.
    void escapeToListRequested();
    void dirtyStateChanged(bool dirty);

private slots:
    void onTextChanged();
    void onTabChanged(int index);
    void onSaveClicked();
    void onRevertClicked();
    void onCancelClicked();

private:
    // QTextBrowser markdown preview is created on first Preview-tab visit.
    // Constructing/painting it under QT_QPA_PLATFORM=offscreen with a failed
    // QOpenGLWidget sibling has segfaulted on CI's Qt 6.4.
    void ensurePreview();
    void updatePreview();
    void updateActions();
    void setDirty(bool dirty);
    void setSourceQuote(const QString &text);
    bool eventFilter(QObject *watched, QEvent *event) override;

    qint64 m_annotationId = 0;
    QString m_sourceText;
    QString m_savedMarkdown;
    bool m_creating = false;
    bool m_dirty = false;
    bool m_loading = false;

    QStackedWidget *m_stack = nullptr;
    QWidget *m_editorPage = nullptr;
    QLabel *m_emptyLabel = nullptr;

    QLabel *m_pageLabel = nullptr;
    QLabel *m_sourceQuote = nullptr;
    QLabel *m_dirtyLabel = nullptr;
    QPlainTextEdit *m_editor = nullptr;
    SafeMarkdownView *m_preview = nullptr;
    QWidget *m_previewPlaceholder = nullptr;
    QTabWidget *m_tabs = nullptr;
    QPushButton *m_saveButton = nullptr;
    QPushButton *m_revertButton = nullptr;
    QPushButton *m_cancelButton = nullptr;
};

} // namespace syodep
