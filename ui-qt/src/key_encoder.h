// Translates Qt key events into syodep's textual chord syntax ("j", "G",
// "<C-d>", "<Esc>", ...). This is the only piece of input knowledge the
// shell has; all interpretation happens in the Rust core.
#pragma once

#include <QKeyEvent>
#include <QString>

namespace syodep {

// Returns an empty string for events that are not chords (pure modifier
// presses, dead keys, ...).
QString encodeKeyEvent(const QKeyEvent *event);

} // namespace syodep
