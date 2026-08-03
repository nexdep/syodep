#include "canvas_widget.h"

#include <QPainter>
#include <QPainterPath>
#include <QWheelEvent>

#include "core_controller.h"
#include "key_encoder.h"

namespace syodep {

CanvasWidget::CanvasWidget(CoreController *core, QWidget *parent)
    : QOpenGLWidget(parent)
    , m_core(core)
    , m_background(QStringLiteral("#1e1e1e"))
    , m_focusColor(0xad, 0xd8, 0xe6, 102)
    , m_visualColor(0xd3, 0xd3, 0xd3, 102)
    , m_highlightColor(0xff, 0xd4, 0x00, 102)
{
    setFocusPolicy(Qt::StrongFocus);

    connect(m_core, &CoreController::redrawRequested, this, qOverload<>(&QWidget::update));
    // Cache invalidation must run before the associated redraw.
    connect(m_core, &CoreController::pageCacheInvalidationRequested,
            this, &CanvasWidget::clearPageCache);
}

void CanvasWidget::resizeGL(int w, int h)
{
    const qreal dpr = devicePixelRatioF();
    m_core->setDevicePixelRatio(float(dpr));
    m_core->setViewportSize(float(w * dpr), float(h * dpr));
    m_pageCache.clear();
}

void CanvasWidget::keyPressEvent(QKeyEvent *event)
{
    const QString chord = encodeKeyEvent(event);
    if (chord.isEmpty()) {
        QOpenGLWidget::keyPressEvent(event);
        return;
    }
    m_core->sendKey(chord);
}

void CanvasWidget::wheelEvent(QWheelEvent *event)
{
    const qreal dpr = devicePixelRatioF();
    // angleDelta is in 1/8 degree; a standard wheel notch (15 deg) scrolls
    // three text-ish lines worth of pixels.
    const QPointF delta = QPointF(event->angleDelta()) / 8.0 / 15.0 * 50.0 * dpr;
    m_core->scrollBy(float(-delta.x()), float(-delta.y()));
}

QImage CanvasWidget::pageImage(size_t page)
{
    auto it = m_pageCache.find(page);
    if (it != m_pageCache.end())
        return it->image;

    QImage owned = m_core->renderPage(page);
    if (owned.isNull())
        return {};

    // Very small bound; the real render cache lives in the core. This only
    // avoids re-copying bitmaps across the FFI on every repaint.
    if (m_pageCache.size() > 8)
        m_pageCache.clear();
    m_pageCache.insert(page, CachedPage{owned, 0.0});
    return owned;
}

void CanvasWidget::paintGL()
{
    QPainter painter(this);
    painter.fillRect(rect(), m_background);

    if (!m_core->hasDocument())
        return;

    const qreal dpr = devicePixelRatioF();

    const QVector<CoreVisiblePage> pages = m_core->visiblePages();
    for (const CoreVisiblePage &vp : pages) {
        // Invalidate the cached image when the zoom changed: the bitmap the
        // core would render no longer matches the cached resolution.
        auto it = m_pageCache.find(vp.page);
        if (it != m_pageCache.end()
            && qAbs(qreal(it->image.width()) - vp.pixelRect.width()) > 1.5) {
            m_pageCache.erase(it);
        }

        const QImage image = pageImage(vp.page);
        if (image.isNull())
            continue;
        const QRectF target(
            vp.pixelRect.x() / dpr,
            vp.pixelRect.y() / dpr,
            vp.pixelRect.width() / dpr,
            vp.pixelRect.height() / dpr);
        painter.drawImage(target, image);
    }

    // Overlays. At most one of focus and visual is ever valid -- they are
    // different modes -- while highlights can coexist with either, since stored
    // highlights are drawn whatever the mode.
    //
    // Every rectangle goes into a QPainterPath that is simplified before a
    // single fill. simplified() merges intersecting subpaths into an outline
    // with no intersecting edges, so overlapping boxes are painted once at a
    // uniform opacity. Filling each rectangle separately would blend the
    // overlaps twice and band the result -- and they do overlap: the rects
    // come from MuPDF line bounds, which include ascenders and descenders.
    //
    // No borders anywhere: a multi-line highlight outlined per rectangle shows
    // a ladder of internal edges where the lines meet.
    const auto addRect = [dpr](QPainterPath &path, const QRectF &pixelRect) {
        QRectF box(
            pixelRect.x() / dpr,
            pixelRect.y() / dpr,
            pixelRect.width() / dpr,
            pixelRect.height() / dpr);
        // Keep zero-size stops (spaces, empty lines) visible. Applied after the
        // dpr division, so the minimum is 2 logical pixels.
        if (box.width() < 2.0)
            box.setWidth(2.0);
        if (box.height() < 2.0)
            box.setHeight(2.0);
        path.addRect(box);
    };

    const auto fillOverlay = [&](const CoreOverlay &overlay, const QColor &color) {
        QPainterPath path;
        if (overlay.valid) {
            for (const QRectF &r : overlay.pixelRects)
                addRect(path, r);
        }
        if (!path.isEmpty())
            painter.fillPath(path.simplified(), color);
    };

    // Highlights are painted differently from focus/visual: a saved highlight
    // is always rendered with Multiply blending (every PDF reader does this
    // for a Highlight annotation, and the core writes the same opacity into
    // the saved `/CA` -- see `syo_app_highlight_color`), so previewing it with
    // plain alpha blending, as focus/visual use, would make it look paler on
    // screen than it does once saved.
    //
    // `QPainter::CompositionMode_Multiply` is not used directly on this
    // widget's own painter: `QOpenGLWidget` paints through the GL paint
    // engine, where the advanced (SVG/PDF-spec) blend modes depend on an
    // OpenGL blend-equation extension that is not universal -- confirmed by
    // testing, where a GPU/driver without it silently painted solid black
    // instead of blending at all. Compositing on a `QImage` first uses the
    // raster paint engine instead, where these blend modes are unconditionally
    // correct, and the already-blended pixels are then just drawn like a page
    // image -- no blend mode needed for that part.
    {
        const QVector<CoreHighlightOverlay> groups = m_core->highlightOverlays();
        for (const CoreHighlightOverlay &group : groups) {
            for (const QRectF &r : group.pixelRects) {
                // Overlay rects and visible-page rects share canvas-pixel space.
                // Select by overlap (not strict containment): MuPDF line bounds
                // routinely poke past the media box, and float rounding can push
                // an edge-touching rect out by a fraction of a pixel — those used
                // to paint nowhere. A rect straddling the page gap paints its
                // part on each overlapping page; the per-page intersections are
                // disjoint, so nothing double-blends.
                for (const CoreVisiblePage &vp : pages) {
                    const QRectF overlap = r.intersected(vp.pixelRect);
                    if (overlap.isEmpty())
                        continue;
                    const QImage page = pageImage(vp.page);
                    if (page.isNull())
                        continue;
                    QRect local(
                        qRound(overlap.x() - vp.pixelRect.x()),
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
                    const QRectF target(
                        (vp.pixelRect.x() + local.x()) / dpr,
                        (vp.pixelRect.y() + local.y()) / dpr,
                        local.width() / dpr,
                        local.height() / dpr);
                    painter.drawImage(target, patch);
                }
            }
        }
    }

    fillOverlay(m_core->focusOverlay(), m_focusColor);
    fillOverlay(m_core->selectionOverlay(), m_visualColor);
}

} // namespace syodep
