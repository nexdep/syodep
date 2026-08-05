// Renderer-neutral canvas interface for the thin Qt shell.
//
// Both implementations paint the same core-provided page bitmaps and forward
// the same input. OpenGL is a presentation backend, never a home for document
// or navigation logic; the raster backend is the reliable Wayland fallback.
#pragma once

#include <QColor>

class QWidget;

namespace syodep {

class CoreController;

enum class RendererBackend
{
    OpenGl,
    Raster,
};

// Non-QObject interface implemented alongside exactly one QWidget base by each
// concrete canvas. The returned QWidget is owned by MainWindow through Qt's
// normal parent/central-widget ownership.
class CanvasWidget
{
public:
    virtual ~CanvasWidget() = default;

    virtual QWidget *widget() = 0;
    virtual void setBackgroundColor(const QColor &color) = 0;
    virtual void setFocusColor(const QColor &color) = 0;
    virtual void setVisualColor(const QColor &color) = 0;
    virtual void setHighlightColor(const QColor &color) = 0;
};

CanvasWidget *createCanvasWidget(RendererBackend backend,
                                 CoreController *core,
                                 QWidget *parent = nullptr);

} // namespace syodep
