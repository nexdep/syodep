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
    m_stack->addWidget(m_highlights);
    root->addWidget(m_stack);

    connect(m_highlights, &HighlightsPanel::focusCanvasRequested,
            this, &AnnotationSidebar::focusCanvasRequested);

    // Owned here so File → Export stays available before the Annotations page
    // has been realized. The panel is created on first trigger / page show.
    m_annotationsExportAction = new QAction(tr("Export annotations as Markdown…"), this);
    connect(m_annotationsExportAction, &QAction::triggered, this, [this]() {
        ensureAnnotationsPanel();
        m_annotations->exportAction()->trigger();
    });

    showPage(SidebarPage::Highlights);
}

void AnnotationSidebar::ensureAnnotationsPanel()
{
    if (m_annotations)
        return;

    m_annotations = new AnnotationsPanel(m_core, m_stack);
    m_stack->addWidget(m_annotations);
    connect(m_annotations, &AnnotationsPanel::focusCanvasRequested,
            this, &AnnotationSidebar::focusCanvasRequested);
}

void AnnotationSidebar::showPage(SidebarPage page)
{
    clearPendingKeys();
    m_activePage = page;
    if (page == SidebarPage::Annotations) {
        ensureAnnotationsPanel();
        m_stack->setCurrentWidget(m_annotations);
    } else {
        m_stack->setCurrentWidget(m_highlights);
    }
}

void AnnotationSidebar::focusActivePage()
{
    if (m_activePage == SidebarPage::Highlights) {
        m_highlights->focusList();
        return;
    }
    ensureAnnotationsPanel();
    if (m_annotations->isCreating() || m_annotations->isDirty())
        m_annotations->focusEditor();
    else
        m_annotations->focusList();
}

void AnnotationSidebar::beginAnnotationCreation()
{
    ensureAnnotationsPanel();
    showPage(SidebarPage::Annotations);
    m_annotations->beginCreation();
}

bool AnnotationSidebar::isDirty() const
{
    return m_annotations && m_annotations->isDirty();
}

bool AnnotationSidebar::confirmDiscardDirty(const QString &actionLabel)
{
    if (!m_annotations)
        return true;
    return m_annotations->confirmDiscardDirty(actionLabel);
}

void AnnotationSidebar::clearPendingKeys()
{
    m_highlights->clearPendingKey();
    if (m_annotations)
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
