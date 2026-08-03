// Shared j/k, gg/G, pending g/d, Escape→canvas handling for sidebar lists.
#pragma once

#include <QKeyEvent>
#include <QTimer>
#include <functional>

namespace syodep {

enum class SidebarPendingKey {
    None,
    G,
    D
};

struct SidebarListKeyActions
{
    std::function<void(int row)> selectRow;
    std::function<void()> deleteSelected;
    std::function<void()> copySource;
    std::function<void()> copyMarkdown;
    std::function<void()> reveal;
    std::function<void()> escapeToCanvas;
    // Optional page-specific action (Annotations: edit). Null = ignore `e`.
    std::function<void()> edit;
};

inline void clearSidebarPendingKey(SidebarPendingKey *pending, QTimer *timer)
{
    *pending = SidebarPendingKey::None;
    if (timer)
        timer->stop();
}

// Returns true when the key was handled for the list.
inline bool handleSidebarListKey(QKeyEvent *event,
                                 int rows,
                                 int currentRow,
                                 SidebarPendingKey *pending,
                                 QTimer *pendingTimer,
                                 const SidebarListKeyActions &actions)
{
    if (!event || !pending)
        return false;

    const int key = event->key();
    const Qt::KeyboardModifiers mods = event->modifiers();
    const bool plain = (mods & ~Qt::KeypadModifier) == Qt::NoModifier;
    const bool shifted = (mods & ~Qt::KeypadModifier) == Qt::ShiftModifier;

    if (*pending != SidebarPendingKey::None) {
        const SidebarPendingKey was = *pending;
        clearSidebarPendingKey(pending, pendingTimer);
        if (was == SidebarPendingKey::G && key == Qt::Key_G && plain) {
            if (actions.selectRow)
                actions.selectRow(0);
            return true;
        }
        if (was == SidebarPendingKey::D && key == Qt::Key_D && plain) {
            if (actions.deleteSelected)
                actions.deleteSelected();
            return true;
        }
        // Not the partner key: fall through and let it start something new.
    }

    if (plain) {
        switch (key) {
        case Qt::Key_J:
        case Qt::Key_Down:
            if (rows > 0 && actions.selectRow)
                actions.selectRow(currentRow < 0 ? 0 : qMin(currentRow + 1, rows - 1));
            return true;
        case Qt::Key_K:
        case Qt::Key_Up:
            if (rows > 0 && actions.selectRow)
                actions.selectRow(currentRow <= 0 ? 0 : currentRow - 1);
            return true;
        case Qt::Key_Home:
            if (actions.selectRow)
                actions.selectRow(0);
            return true;
        case Qt::Key_End:
            if (actions.selectRow)
                actions.selectRow(rows - 1);
            return true;
        case Qt::Key_G:
            *pending = SidebarPendingKey::G;
            if (pendingTimer)
                pendingTimer->start();
            return true;
        case Qt::Key_D:
            *pending = SidebarPendingKey::D;
            if (pendingTimer)
                pendingTimer->start();
            return true;
        case Qt::Key_Y:
            if (actions.copySource)
                actions.copySource();
            return true;
        case Qt::Key_E:
            if (actions.edit) {
                actions.edit();
                return true;
            }
            break;
        case Qt::Key_Return:
        case Qt::Key_Enter:
            if (actions.reveal)
                actions.reveal();
            return true;
        case Qt::Key_Escape:
            if (actions.escapeToCanvas)
                actions.escapeToCanvas();
            return true;
        default:
            break;
        }
    }

    if (shifted) {
        switch (key) {
        case Qt::Key_G:
            if (actions.selectRow)
                actions.selectRow(rows - 1);
            return true;
        case Qt::Key_Y:
            if (actions.copyMarkdown)
                actions.copyMarkdown();
            return true;
        default:
            break;
        }
    }

    return false;
}

} // namespace syodep
