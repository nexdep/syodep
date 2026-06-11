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

#include "syodep_ffi.h"

namespace syodep {

class CanvasWidget : public QOpenGLWidget
{
    Q_OBJECT
public:
    explicit CanvasWidget(SyoApp *app, QWidget *parent = nullptr);

    void setBackgroundColor(const QColor &color) { m_background = color; }

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

    SyoApp *m_app; // owned by MainWindow
    QColor m_background;

    struct CachedPage
    {
        QImage image;
        qreal zoomKey = 0.0;
    };
    QHash<size_t, CachedPage> m_pageCache;
    qreal m_lastZoomKey = 0.0;
};

} // namespace syodep
