// Selected-highlight Markdown comment editor (one editor for the sidebar).
#pragma once

#include <QWidget>

#include "core_controller.h"

class QLabel;
class QPlainTextEdit;
class QPushButton;
class QTabWidget;
class QStackedWidget;

namespace syodep {

class SafeMarkdownView;

class HighlightCommentEditor final : public QWidget
{
    Q_OBJECT

public:
    explicit HighlightCommentEditor(QWidget *parent = nullptr);

    void loadHighlight(const HighlightListItem &item);
    void clearHighlight();
    void showUnavailableMessage(const QString &message);

    bool isDirty() const;
    bool hasHighlight() const { return m_highlightId != 0; }
    qint64 highlightId() const { return m_highlightId; }
    QString markdown() const;
    QString savedMarkdown() const { return m_savedMarkdown; }

    void markSaved(const QString &savedBody);
    void revert();
    // Reload metadata from the item while keeping an unsaved draft for the
    // same highlight id (used when the list refreshes underfoot).
    void reloadPreservingDraft(const HighlightListItem &item,
                               const QString &savedBaseline,
                               const QString &draft);

signals:
    void saveRequested(qint64 highlightId, const QString &markdown);
    void dirtyStateChanged(bool dirty);

private slots:
    void onTextChanged();
    void onTabChanged(int index);
    void onSaveClicked();
    void onRevertClicked();

private:
    void updatePreview();
    void updateActions();
    void setDirty(bool dirty);

    qint64 m_highlightId = 0;
    QString m_savedMarkdown;
    bool m_dirty = false;
    bool m_loading = false;

    QStackedWidget *m_stack = nullptr;
    QWidget *m_editorPage = nullptr;
    QLabel *m_emptyLabel = nullptr;

    QLabel *m_pageLabel = nullptr;
    QLabel *m_stateLabel = nullptr;
    QLabel *m_sourceQuote = nullptr;
    QLabel *m_dirtyLabel = nullptr;
    QPlainTextEdit *m_editor = nullptr;
    SafeMarkdownView *m_preview = nullptr;
    QTabWidget *m_tabs = nullptr;
    QPushButton *m_saveButton = nullptr;
    QPushButton *m_revertButton = nullptr;
};

} // namespace syodep
