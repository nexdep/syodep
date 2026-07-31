// OpenGL-backed canvas that draws what the core asks it to draw.
//
// Responsibilities (and nothing more):
//  - forward key/wheel/resize events through CoreController,
//  - fetch visible page rectangles + bitmaps via CoreController and paint them,
//  - cache uploaded page images per zoom level to avoid redundant FFI copies.
//
// All document/navigation logic lives in the Rust core. Effect-bit decoding and
// SyoApp* ownership live in CoreController.
#pragma once

#include <QHash>
#include <QImage>
#include <QOpenGLWidget>

class QWheelEvent;
class QKeyEvent;

namespace syodep {

class CoreController;

class CanvasWidget : public QOpenGLWidget
{
    Q_OBJECT
public:
    explicit CanvasWidget(CoreController *core, QWidget *parent = nullptr);

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

protected:
    void paintGL() override;
    void resizeGL(int w, int h) override;
    void keyPressEvent(QKeyEvent *event) override;
    void wheelEvent(QWheelEvent *event) override;

private:
    QImage pageImage(size_t page);

    CoreController *m_core = nullptr; // non-owning; controller outlives canvas

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
};

} // namespace syodep
