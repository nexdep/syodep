#include "sidebar/annotation_sidebar.h"

#include <QVBoxLayout>
#include <QStackedWidget>
#include <QListView>
#include <QAction>

#include "core_controller.h"
#include "sidebar/highlights_panel.h"
#include "sidebar/annotations_panel.h"

namespace syodep {

AnnotationSidebar::AnnotationSidebar(CoreController *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
{
    Q_ASSERT(m_core);
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);

    m_stack = new QStackedWidget(this);
    m_highlights = new HighlightsPanel(m_core, m_stack);
    m_annotations = new AnnotationsPanel(m_core, m_stack);
    m_stack->addWidget(m_highlights);
    m_stack->addWidget(m_annotations);
    root->addWidget(m_stack);

    connect(m_highlights, &HighlightsPanel::focusCanvasRequested,
            this, &AnnotationSidebar::focusCanvasRequested);
    connect(m_annotations, &AnnotationsPanel::focusCanvasRequested,
            this, &AnnotationSidebar::focusCanvasRequested);

    showPage(SidebarPage::Highlights);
}

void AnnotationSidebar::showPage(SidebarPage page)
{
    clearPendingKeys();
    m_activePage = page;
    m_stack->setCurrentWidget(page == SidebarPage::Highlights
                                  ? static_cast<QWidget *>(m_highlights)
                                  : static_cast<QWidget *>(m_annotations));
}

void AnnotationSidebar::focusActivePage()
{
    if (m_activePage == SidebarPage::Highlights) {
        m_highlights->focusList();
        return;
    }
    if (m_annotations->isCreating() || m_annotations->isDirty())
        m_annotations->focusEditor();
    else
        m_annotations->focusList();
}

void AnnotationSidebar::beginAnnotationCreation()
{
    showPage(SidebarPage::Annotations);
    m_annotations->beginCreation();
}

bool AnnotationSidebar::isDirty() const
{
    return m_annotations && m_annotations->isDirty();
}

bool AnnotationSidebar::confirmDiscardDirty(const QString &actionLabel)
{
    return m_annotations->confirmDiscardDirty(actionLabel);
}

void AnnotationSidebar::clearPendingKeys()
{
    m_highlights->clearPendingKey();
    m_annotations->clearPendingKey();
}

void AnnotationSidebar::refreshAnnotations(bool force)
{
    m_highlights->refreshAnnotations(force);
}

AnnotationSidebar::ContentState AnnotationSidebar::contentState() const
{
    switch (m_highlights->contentState()) {
    case HighlightsPanel::ContentState::NoDocument:
        return ContentState::NoDocument;
    case HighlightsPanel::ContentState::EmptyHighlights:
        return ContentState::EmptyHighlights;
    case HighlightsPanel::ContentState::HighlightList:
        return ContentState::HighlightList;
    }
    return ContentState::NoDocument;
}

HighlightListModel *AnnotationSidebar::model() const
{
    return m_highlights->model();
}

QListView *AnnotationSidebar::listView() const
{
    return m_highlights->listView();
}

QAction *AnnotationSidebar::exportAction() const
{
    return m_highlights->exportAction();
}

void AnnotationSidebar::focusList()
{
    focusActivePage();
}

} // namespace syodep
