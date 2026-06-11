#include "key_encoder.h"

namespace syodep {
namespace {

// Named keys understood by syodep-config's chord parser.
const char *namedKey(int key)
{
    switch (key) {
    case Qt::Key_Escape: return "Esc";
    case Qt::Key_Return:
    case Qt::Key_Enter: return "CR";
    case Qt::Key_Tab:
    case Qt::Key_Backtab: return "Tab";
    case Qt::Key_Space: return "Space";
    case Qt::Key_Backspace: return "BS";
    case Qt::Key_Up: return "Up";
    case Qt::Key_Down: return "Down";
    case Qt::Key_Left: return "Left";
    case Qt::Key_Right: return "Right";
    case Qt::Key_PageUp: return "PageUp";
    case Qt::Key_PageDown: return "PageDown";
    case Qt::Key_Home: return "Home";
    case Qt::Key_End: return "End";
    default: return nullptr;
    }
}

} // namespace

QString encodeKeyEvent(const QKeyEvent *event)
{
    const int key = event->key();
    if (key == Qt::Key_Shift || key == Qt::Key_Control || key == Qt::Key_Alt
        || key == Qt::Key_Meta || key == Qt::Key_AltGr || key == Qt::Key_CapsLock
        || key == Qt::Key_unknown) {
        return {};
    }

    const bool ctrl = event->modifiers().testFlag(Qt::ControlModifier);
    const bool alt = event->modifiers().testFlag(Qt::AltModifier);

    QString name;
    bool needsBrackets = ctrl || alt;
    if (const char *named = namedKey(key)) {
        name = QLatin1String(named);
        needsBrackets = true;
    } else if (!ctrl && !event->text().isEmpty() && event->text().at(0).isPrint()) {
        // The event text already reflects shift and keyboard layout.
        name = event->text().at(0);
    } else if (key >= 0x20 && key <= 0x7e) {
        // With ctrl held, text() is a control character; reconstruct the
        // character from the key code. Qt reports letters as uppercase.
        QChar c(key);
        name = event->modifiers().testFlag(Qt::ShiftModifier) ? c.toUpper() : c.toLower();
    } else {
        return {};
    }

    if (!needsBrackets)
        return name;

    QString out = QStringLiteral("<");
    if (ctrl)
        out += QLatin1String("C-");
    if (alt)
        out += QLatin1String("A-");
    out += name;
    out += QLatin1Char('>');
    return out;
}

} // namespace syodep
