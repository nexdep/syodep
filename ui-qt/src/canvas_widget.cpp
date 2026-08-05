#include "canvas_widget.h"

#include <QHash>
#include <QImage>
#include <QKeyEvent>
#include <QOpenGLWidget>
#include <QPainter>
#include <QPainterPath>
#include <QResizeEvent>
#include <QWheelEvent>
#include <QWidget>

#include "core_controller.h"
#include "key_encoder.h"

namespace syodep {

namespace {

// All backend-independent presentation state lives here so the OpenGL and
// raster widgets cannot drift in what they draw or which events they forward.
class CanvasState
{
public:
    explicit CanvasState(CoreController *core)
        : m_core(core)
        , m_background(QStringLiteral("#1e1e1e"))
        , m_focusColor(0xad, 0xd8, 0xe6, 102)
        , m_visualColor(0xd3, 0xd3, 0xd3, 102)
        , m_highlightColor(0xff, 0xd4, 0x00, 102)
    {
    }

    void connectTo(QWidget *widget)
    {
        widget->setFocusPolicy(Qt::StrongFocus);
        QObject::connect(m_core, &CoreController::redrawRequested,
                         widget, qOverload<>(&QWidget::update));
        QObject::connect(m_core, &CoreController::pageCacheInvalidationRequested,
                         widget, [this]() { m_pageCache.clear(); });
    }

    void resize(int width, int height, qreal dpr)
    {
        m_core->setDevicePixelRatio(float(dpr));
        m_core->setViewportSize(float(width * dpr), float(height * dpr));
        m_pageCache.clear();
    }

    bool keyPress(QKeyEvent *event)
    {
        const QString chord = encodeKeyEvent(event);
        if (chord.isEmpty())
            return false;
        m_core->sendKey(chord);
        return true;
    }

    void wheel(QWheelEvent *event, qreal dpr)
    {
        // angleDelta is in 1/8 degree; a standard wheel notch (15 deg) scrolls
        // three text-ish lines worth of pixels.
        const QPointF delta = QPointF(event->angleDelta()) / 8.0 / 15.0 * 50.0 * dpr;
        m_core->scrollBy(float(-delta.x()), float(-delta.y()));
    }

    void paint(QPainter &painter, const QWidget *widget)
    {
        painter.fillRect(widget->rect(), m_background);

        if (!m_core->hasDocument())
            return;

        const qreal dpr = widget->devicePixelRatioF();
        const QVector<CoreVisiblePage> pages = m_core->visiblePages();
        for (const CoreVisiblePage &vp : pages) {
            // Invalidate the cached image when zoom changed: the bitmap the
            // core would render no longer matches the cached resolution.
            auto it = m_pageCache.find(vp.page);
            if (it != m_pageCache.end()
                && qAbs(qreal(it->width()) - vp.pixelRect.width()) > 1.5) {
                m_pageCache.erase(it);
            }

            const QImage image = pageImage(vp.page);
            if (image.isNull())
                continue;
            const QRectF target(vp.pixelRect.x() / dpr,
                                vp.pixelRect.y() / dpr,
                                vp.pixelRect.width() / dpr,
                                vp.pixelRect.height() / dpr);
            painter.drawImage(target, image);
        }

        // Focus/visual rectangles share one simplified path so overlap is
        // painted once. Highlight Multiply blending is performed on QImage
        // patches, where Qt's raster engine is dependable on both backends.
        const auto addRect = [dpr](QPainterPath &path, const QRectF &pixelRect) {
            QRectF box(pixelRect.x() / dpr,
                       pixelRect.y() / dpr,
                       pixelRect.width() / dpr,
                       pixelRect.height() / dpr);
            if (box.width() < 2.0)
                box.setWidth(2.0);
            if (box.height() < 2.0)
                box.setHeight(2.0);
            path.addRect(box);
        };

        const auto fillOverlay = [&](const CoreOverlay &overlay, const QColor &color) {
            QPainterPath path;
            if (overlay.valid) {
                for (const QRectF &rect : overlay.pixelRects)
                    addRect(path, rect);
            }
            if (!path.isEmpty())
                painter.fillPath(path.simplified(), color);
        };

        const QVector<CoreHighlightOverlay> groups = m_core->highlightOverlays();
        for (const CoreHighlightOverlay &group : groups) {
            for (const QRectF &rect : group.pixelRects) {
                for (const CoreVisiblePage &vp : pages) {
                    const QRectF overlap = rect.intersected(vp.pixelRect);
                    if (overlap.isEmpty())
                        continue;
                    const QImage page = pageImage(vp.page);
                    if (page.isNull())
                        continue;
                    QRect local(qRound(overlap.x() - vp.pixelRect.x()),
                                qRound(overlap.y() - vp.pixelRect.y()),
                                qMax(1, qRound(overlap.width())),
                                qMax(1, qRound(overlap.height())));
                    local = local.intersected(page.rect());
                    if (local.isEmpty())
                        continue;
                    QImage patch = page.copy(local);
                    QPainter patchPainter(&patch);
                    patchPainter.setCompositionMode(QPainter::CompositionMode_Multiply);
                    patchPainter.fillRect(patch.rect(), group.color);
                    patchPainter.end();
                    const QRectF target((vp.pixelRect.x() + local.x()) / dpr,
                                        (vp.pixelRect.y() + local.y()) / dpr,
                                        local.width() / dpr,
                                        local.height() / dpr);
                    painter.drawImage(target, patch);
                }
            }
        }

        fillOverlay(m_core->focusOverlay(), m_focusColor);
        fillOverlay(m_core->selectionOverlay(), m_visualColor);
    }

    void setBackgroundColor(const QColor &color) { m_background = color; }
    void setFocusColor(const QColor &color) { m_focusColor = color; }
    void setVisualColor(const QColor &color) { m_visualColor = color; }
    void setHighlightColor(const QColor &color) { m_highlightColor = color; }

private:
    QImage pageImage(size_t page)
    {
        auto it = m_pageCache.find(page);
        if (it != m_pageCache.end())
            return *it;

        QImage owned = m_core->renderPage(page);
        if (owned.isNull())
            return {};
        if (m_pageCache.size() > 8)
            m_pageCache.clear();
        m_pageCache.insert(page, owned);
        return owned;
    }

    CoreController *m_core = nullptr; // non-owning; controller outlives canvas
    QColor m_background;
    QColor m_focusColor;
    QColor m_visualColor;
    QColor m_highlightColor;
    QHash<size_t, QImage> m_pageCache;
};

class OpenGlCanvasWidget final : public QOpenGLWidget, public CanvasWidget
{
public:
    OpenGlCanvasWidget(CoreController *core, QWidget *parent)
        : QOpenGLWidget(parent)
        , m_state(core)
    {
        m_state.connectTo(this);
    }

    QWidget *widget() override { return this; }
    void setBackgroundColor(const QColor &color) override { m_state.setBackgroundColor(color); }
    void setFocusColor(const QColor &color) override { m_state.setFocusColor(color); }
    void setVisualColor(const QColor &color) override { m_state.setVisualColor(color); }
    void setHighlightColor(const QColor &color) override { m_state.setHighlightColor(color); }

protected:
    void paintGL() override
    {
        QPainter painter(this);
        m_state.paint(painter, this);
    }

    void resizeGL(int width, int height) override
    {
        m_state.resize(width, height, devicePixelRatioF());
    }

    void keyPressEvent(QKeyEvent *event) override
    {
        if (!m_state.keyPress(event))
            QOpenGLWidget::keyPressEvent(event);
    }

    void wheelEvent(QWheelEvent *event) override
    {
        m_state.wheel(event, devicePixelRatioF());
    }

private:
    CanvasState m_state;
};

class RasterCanvasWidget final : public QWidget, public CanvasWidget
{
public:
    RasterCanvasWidget(CoreController *core, QWidget *parent)
        : QWidget(parent)
        , m_state(core)
    {
        m_state.connectTo(this);
    }

    QWidget *widget() override { return this; }
    void setBackgroundColor(const QColor &color) override { m_state.setBackgroundColor(color); }
    void setFocusColor(const QColor &color) override { m_state.setFocusColor(color); }
    void setVisualColor(const QColor &color) override { m_state.setVisualColor(color); }
    void setHighlightColor(const QColor &color) override { m_state.setHighlightColor(color); }

protected:
    void paintEvent(QPaintEvent *) override
    {
        QPainter painter(this);
        m_state.paint(painter, this);
    }

    void resizeEvent(QResizeEvent *event) override
    {
        QWidget::resizeEvent(event);
        m_state.resize(event->size().width(), event->size().height(), devicePixelRatioF());
    }

    void keyPressEvent(QKeyEvent *event) override
    {
        if (!m_state.keyPress(event))
            QWidget::keyPressEvent(event);
    }

    void wheelEvent(QWheelEvent *event) override
    {
        m_state.wheel(event, devicePixelRatioF());
    }

private:
    CanvasState m_state;
};

} // namespace

CanvasWidget *createCanvasWidget(RendererBackend backend,
                                 CoreController *core,
                                 QWidget *parent)
{
    if (backend == RendererBackend::Raster)
        return new RasterCanvasWidget(core, parent);
    return new OpenGlCanvasWidget(core, parent);
}

} // namespace syodep
