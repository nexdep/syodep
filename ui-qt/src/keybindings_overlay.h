// Modal, scrollable presentation of the core's effective keybindings.
//
// The widget owns no command semantics: it forwards encoded keys through the
// controller and applies the help-navigation effects the core returns.
#pragma once

#include <QWidget>

#include "core_controller.h"

class QScrollArea;
class QVBoxLayout;
class QWheelEvent;
class QKeyEvent;

namespace syodep {

class KeybindingsOverlay final : public QWidget
{
    Q_OBJECT

public:
    explicit KeybindingsOverlay(CoreController *core, QWidget *parent = nullptr);

    void setSnapshot(const KeybindingSnapshot &snapshot);
    void navigate(HelpNavigation navigation);
    qsizetype bindingCount() const { return m_bindingCount; }
    int scrollValue() const;
    int maximumScrollValue() const;

protected:
    void keyPressEvent(QKeyEvent *event) override;
    void wheelEvent(QWheelEvent *event) override;

private:
    CoreController *m_core = nullptr; // non-owning; controller outlives overlay
    QScrollArea *m_scroll = nullptr;
    QWidget *m_sections = nullptr;
    QVBoxLayout *m_sectionsLayout = nullptr;
    qsizetype m_bindingCount = 0;
};

} // namespace syodep
