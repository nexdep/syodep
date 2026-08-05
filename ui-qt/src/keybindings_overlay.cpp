#include "keybindings_overlay.h"

#include <QFrame>
#include <QGridLayout>
#include <QKeyEvent>
#include <QLabel>
#include <QScrollArea>
#include <QScrollBar>
#include <QVBoxLayout>
#include <QWheelEvent>

#include "key_encoder.h"

namespace syodep {

namespace {

QString groupName(KeybindingGroup group)
{
    switch (group) {
    case KeybindingGroup::Help: return QObject::tr("Help overlay controls");
    case KeybindingGroup::Common: return QObject::tr("Common");
    case KeybindingGroup::Normal: return QObject::tr("Normal");
    case KeybindingGroup::Focus: return QObject::tr("Focus");
    case KeybindingGroup::Visual: return QObject::tr("Visual");
    case KeybindingGroup::Highlight: return QObject::tr("Highlight");
    }
    return {};
}

bool isActiveGroup(KeybindingGroup group, DocumentMode activeMode)
{
    return (group == KeybindingGroup::Normal && activeMode == DocumentMode::Normal)
        || (group == KeybindingGroup::Focus && activeMode == DocumentMode::Focus)
        || (group == KeybindingGroup::Visual && activeMode == DocumentMode::Visual)
        || (group == KeybindingGroup::Highlight && activeMode == DocumentMode::Highlight);
}

} // namespace

KeybindingsOverlay::KeybindingsOverlay(CoreController *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
{
    setObjectName(QStringLiteral("keybindingsOverlay"));
    setFocusPolicy(Qt::StrongFocus);
    setAttribute(Qt::WA_StyledBackground, true);
    setStyleSheet(QStringLiteral(
        "#keybindingsOverlay { background-color: rgba(0, 0, 0, 168); }"
        "#keybindingsPanel { background-color: rgba(28, 30, 34, 242);"
        " border: 1px solid rgba(255, 255, 255, 45); border-radius: 12px; }"
        "#keybindingsTitle { color: #f4f4f4; font-size: 22px; font-weight: 600; }"
        "#keybindingsHint { color: #b8bcc4; }"
        "#keybindingsSection { background-color: rgba(255, 255, 255, 10);"
        " border-radius: 7px; }"
        "#keybindingsSectionTitle { color: #f0f0f0; font-size: 16px; font-weight: 600; }"
        "#keybindingsKeys { color: #f2cc60; font-family: monospace; font-weight: 600; }"
        "#keybindingsDescription { color: #eeeeee; }"
        "#keybindingsCommand { color: #9298a2; font-family: monospace; font-size: 11px; }"));

    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(48, 36, 48, 36);

    auto *panel = new QFrame(this);
    panel->setObjectName(QStringLiteral("keybindingsPanel"));
    panel->setMaximumWidth(980);
    auto *panelLayout = new QVBoxLayout(panel);
    panelLayout->setContentsMargins(24, 20, 24, 20);
    panelLayout->setSpacing(10);

    auto *title = new QLabel(tr("Keybindings"), panel);
    title->setObjectName(QStringLiteral("keybindingsTitle"));
    panelLayout->addWidget(title);
    auto *hint = new QLabel(
        tr("Esc or the configured help shortcut closes this overlay. "
           "Use j/k, arrows, Ctrl-d/u, Ctrl-f/b, PageUp/PageDown, gg/G, "
           "the mouse wheel, or the scrollbar to navigate."),
        panel);
    hint->setObjectName(QStringLiteral("keybindingsHint"));
    hint->setWordWrap(true);
    panelLayout->addWidget(hint);

    m_scroll = new QScrollArea(panel);
    m_scroll->setWidgetResizable(true);
    m_scroll->setFrameShape(QFrame::NoFrame);
    m_scroll->setFocusPolicy(Qt::NoFocus);
    m_scroll->viewport()->setFocusPolicy(Qt::NoFocus);
    m_scroll->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    m_scroll->verticalScrollBar()->setSingleStep(36);
    m_sections = new QWidget(m_scroll);
    m_sectionsLayout = new QVBoxLayout(m_sections);
    m_sectionsLayout->setContentsMargins(0, 4, 0, 4);
    m_sectionsLayout->setSpacing(10);
    m_scroll->setWidget(m_sections);
    panelLayout->addWidget(m_scroll, 1);

    root->addWidget(panel, 1, Qt::AlignHCenter);
    hide();
}

void KeybindingsOverlay::setSnapshot(const KeybindingSnapshot &snapshot)
{
    while (QLayoutItem *item = m_sectionsLayout->takeAt(0)) {
        delete item->widget();
        delete item;
    }
    m_bindingCount = snapshot.items.size();

    const KeybindingGroup groups[] = {
        KeybindingGroup::Help,
        KeybindingGroup::Common,
        KeybindingGroup::Normal,
        KeybindingGroup::Focus,
        KeybindingGroup::Visual,
        KeybindingGroup::Highlight,
    };
    for (KeybindingGroup group : groups) {
        QVector<KeybindingItem> rows;
        for (const KeybindingItem &item : snapshot.items) {
            if (item.group == group)
                rows.push_back(item);
        }
        if (rows.isEmpty())
            continue;

        auto *section = new QFrame(m_sections);
        section->setObjectName(QStringLiteral("keybindingsSection"));
        auto *sectionLayout = new QVBoxLayout(section);
        sectionLayout->setContentsMargins(14, 10, 14, 12);
        sectionLayout->setSpacing(7);
        QString heading = groupName(group);
        if (isActiveGroup(group, snapshot.activeMode))
            heading += tr("  ·  current mode");
        auto *sectionTitle = new QLabel(heading, section);
        sectionTitle->setObjectName(QStringLiteral("keybindingsSectionTitle"));
        sectionLayout->addWidget(sectionTitle);

        auto *grid = new QGridLayout;
        grid->setHorizontalSpacing(22);
        grid->setVerticalSpacing(7);
        grid->setColumnStretch(1, 1);
        for (qsizetype row = 0; row < rows.size(); ++row) {
            const KeybindingItem &item = rows.at(row);
            auto *keys = new QLabel(item.keys, section);
            keys->setObjectName(QStringLiteral("keybindingsKeys"));
            keys->setAlignment(Qt::AlignTop | Qt::AlignLeft);
            grid->addWidget(keys, int(row), 0);

            auto *action = new QWidget(section);
            auto *actionLayout = new QVBoxLayout(action);
            actionLayout->setContentsMargins(0, 0, 0, 0);
            actionLayout->setSpacing(1);
            auto *description = new QLabel(item.description, action);
            description->setObjectName(QStringLiteral("keybindingsDescription"));
            description->setWordWrap(true);
            actionLayout->addWidget(description);
            auto *command = new QLabel(item.command, action);
            command->setObjectName(QStringLiteral("keybindingsCommand"));
            actionLayout->addWidget(command);
            grid->addWidget(action, int(row), 1);
        }
        sectionLayout->addLayout(grid);
        m_sectionsLayout->addWidget(section);
    }
    m_sectionsLayout->addStretch(1);
    m_scroll->verticalScrollBar()->setValue(m_scroll->verticalScrollBar()->minimum());
}

void KeybindingsOverlay::navigate(HelpNavigation navigation)
{
    QScrollBar *bar = m_scroll->verticalScrollBar();
    switch (navigation) {
    case HelpNavigation::LineDown:
        bar->triggerAction(QAbstractSlider::SliderSingleStepAdd);
        break;
    case HelpNavigation::LineUp:
        bar->triggerAction(QAbstractSlider::SliderSingleStepSub);
        break;
    case HelpNavigation::HalfPageDown:
        bar->setValue(bar->value() + m_scroll->viewport()->height() / 2);
        break;
    case HelpNavigation::HalfPageUp:
        bar->setValue(bar->value() - m_scroll->viewport()->height() / 2);
        break;
    case HelpNavigation::PageDown:
        bar->triggerAction(QAbstractSlider::SliderPageStepAdd);
        break;
    case HelpNavigation::PageUp:
        bar->triggerAction(QAbstractSlider::SliderPageStepSub);
        break;
    case HelpNavigation::Top:
        bar->setValue(bar->minimum());
        break;
    case HelpNavigation::Bottom:
        bar->setValue(bar->maximum());
        break;
    }
}

int KeybindingsOverlay::scrollValue() const
{
    return m_scroll->verticalScrollBar()->value();
}

int KeybindingsOverlay::maximumScrollValue() const
{
    return m_scroll->verticalScrollBar()->maximum();
}

void KeybindingsOverlay::keyPressEvent(QKeyEvent *event)
{
    const QString chord = encodeKeyEvent(event);
    if (!chord.isEmpty()) {
        m_core->sendKey(chord);
        event->accept();
        return;
    }
    event->accept();
}

void KeybindingsOverlay::wheelEvent(QWheelEvent *event)
{
    QScrollBar *bar = m_scroll->verticalScrollBar();
    const QPoint delta = event->angleDelta();
    if (!delta.isNull())
        bar->setValue(bar->value() - delta.y());
    else
        bar->setValue(bar->value() - int(event->pixelDelta().y()));
    event->accept();
}

} // namespace syodep
