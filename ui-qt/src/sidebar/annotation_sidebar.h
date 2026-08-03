// Annotation sidebar dock content: Highlights and Annotations pages.
#pragma once

#include <QWidget>

class QStackedWidget;
class QListView;
class QAction;

namespace syodep {

class CoreController;
class HighlightsPanel;
class AnnotationsPanel;
class HighlightListModel;

enum class SidebarPage {
    Highlights,
    Annotations
};

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

    void showPage(SidebarPage page);
    SidebarPage activePage() const { return m_activePage; }

    void focusActivePage();
    void beginAnnotationCreation();
    bool isDirty() const;
    bool confirmDiscardDirty(const QString &actionLabel);
    void clearPendingKeys();

    // Highlights-panel forwards (smoke tests / menus).
    void refreshAnnotations(bool force = false);
    ContentState contentState() const;
    HighlightListModel *model() const;
    QListView *listView() const;
    QAction *exportAction() const;
    void focusList();

    HighlightsPanel *highlightsPanel() const { return m_highlights; }
    AnnotationsPanel *annotationsPanel() const { return m_annotations; }

signals:
    void focusCanvasRequested();

private:
    CoreController *m_core = nullptr;
    QStackedWidget *m_stack = nullptr;
    HighlightsPanel *m_highlights = nullptr;
    AnnotationsPanel *m_annotations = nullptr;
    SidebarPage m_activePage = SidebarPage::Highlights;
};

bool writeHighlightsMarkdown(const QString &path, const QString &markdown, QString *error);

} // namespace syodep
