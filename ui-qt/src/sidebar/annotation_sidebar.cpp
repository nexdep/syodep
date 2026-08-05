#include "sidebar/annotation_sidebar.h"

#include <QApplication>
#include <QEvent>
#include <QKeyEvent>
#include <QPlainTextEdit>
#include <QVBoxLayout>
#include <QStackedWidget>
#include <QListView>
#include <QAction>

#include "core_controller.h"
#include "key_encoder.h"
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

    // Keyboard focus may live on any current or lazily-created sidebar child.
    // A scoped application filter keeps the routing in one place without
    // teaching every list, empty state, preview, and button about commands.
    qApp->installEventFilter(this);

    showPage(SidebarPage::Highlights);
}

bool AnnotationSidebar::eventFilter(QObject *watched, QEvent *event)
{
    if (!m_core || !isVisible() || event->type() != QEvent::KeyPress)
        return QWidget::eventFilter(watched, event);

    auto *target = qobject_cast<QWidget *>(watched);
    if (!target || (target != this && !isAncestorOf(target)))
        return QWidget::eventFilter(watched, event);

    auto *key = static_cast<QKeyEvent *>(event);
    bool insideEditableMarkdown = false;
    for (QWidget *widget = target; widget && widget != this; widget = widget->parentWidget()) {
        if (qobject_cast<QPlainTextEdit *>(widget)) {
            insideEditableMarkdown = true;
            break;
        }
    }
    const bool plainEscape = key->key() == Qt::Key_Escape
        && (key->modifiers() & ~Qt::KeypadModifier) == Qt::NoModifier;
    if (insideEditableMarkdown && !plainEscape)
        return QWidget::eventFilter(watched, event);

    const QString chord = encodeKeyEvent(key);
    if (!chord.isEmpty() && m_core->sendSidebarKey(chord)) {
        key->accept();
        return true;
    }
    return QWidget::eventFilter(watched, event);
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
    m_core->cancelSidebarInput();
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
