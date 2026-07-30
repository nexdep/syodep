// OpenGL-backed canvas that draws what the core asks it to draw.
//
// Responsibilities (and nothing more):
//  - forward key/wheel/resize events to the core,
//  - fetch visible page rectangles + bitmaps from the core and paint them,
//  - cache uploaded page images per zoom level to avoid redundant FFI copies.
//
// All document/navigation logic lives in the Rust core.
#pragma once

#include <cstdint>

#include <QHash>
#include <QImage>
#include <QOpenGLWidget>
#include <QTimer>

#include "syodep_ffi.h"

namespace syodep {

class CanvasWidget : public QOpenGLWidget
{
    Q_OBJECT
public:
    explicit CanvasWidget(SyoApp *app, QWidget *parent = nullptr);

    // Colours come from the config via the core; MainWindow pushes them in
    // after construction. Defaults here only cover the moment before that.
    void setBackgroundColor(const QColor &color) { m_background = color; }
    void setFocusColor(const QColor &color) { m_focusColor = color; }
    void setVisualColor(const QColor &color) { m_visualColor = color; }
    void setHighlightColor(const QColor &color) { m_highlightColor = color; }

    // Drop the cached page images. Needed whenever the bytes behind a page
    // change without its size changing -- opening another document, or a save
    // rewriting this one -- since the cache is otherwise only invalidated by a
    // width mismatch.
    void clearPageCache() { m_pageCache.clear(); }

signals:
    // Emitted after any event was forwarded to the core, so the main window
    // can refresh the status line.
    void coreStateChanged();
    void quitRequested();
    void openFileRequested();

protected:
    void paintGL() override;
    void resizeGL(int w, int h) override;
    void keyPressEvent(QKeyEvent *event) override;
    void wheelEvent(QWheelEvent *event) override;

private:
    void applyEffects(uint32_t effects);
    QImage pageImage(size_t page);

    // A half-typed sequence (`c`, `v`, `o` while selecting) waits for either
    // the next key or a pause. The core owns the decision; the widget only
    // owns the clock, because the core must stay deterministic and testable.
    void updatePendingTimer(uint32_t effects);

    SyoApp *m_app; // owned by MainWindow
    QTimer m_pendingTimer;

    QColor m_background;
    // One colour for every focus mode, one for the selection, one for
    // highlights: the colour says whether focus, a selection or a highlight is
    // active, not which scope.
    QColor m_focusColor;
    QColor m_visualColor;
    QColor m_highlightColor;

    struct CachedPage
    {
        QImage image;
        qreal zoomKey = 0.0;
    };
    QHash<size_t, CachedPage> m_pageCache;
    qreal m_lastZoomKey = 0.0;
};

} // namespace syodep
