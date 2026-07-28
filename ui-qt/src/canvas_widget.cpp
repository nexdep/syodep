#include "canvas_widget.h"

#include <QPainter>
#include <QPainterPath>
#include <QWheelEvent>

#include "key_encoder.h"

namespace syodep {

CanvasWidget::CanvasWidget(SyoApp *app, QWidget *parent)
    : QOpenGLWidget(parent)
    , m_app(app)
    , m_background(QStringLiteral("#1e1e1e"))
    , m_focusColor(0xad, 0xd8, 0xe6, 102)
    , m_visualColor(0xd3, 0xd3, 0xd3, 102)
{
    setFocusPolicy(Qt::StrongFocus);
}

void CanvasWidget::resizeGL(int w, int h)
{
    const qreal dpr = devicePixelRatioF();
    syo_app_set_viewport(m_app, float(w * dpr), float(h * dpr));
    m_pageCache.clear();
    emit coreStateChanged();
}

void CanvasWidget::keyPressEvent(QKeyEvent *event)
{
    const QString chord = encodeKeyEvent(event);
    if (chord.isEmpty()) {
        QOpenGLWidget::keyPressEvent(event);
        return;
    }
    applyEffects(syo_app_key_event(m_app, chord.toUtf8().constData()));
}

void CanvasWidget::wheelEvent(QWheelEvent *event)
{
    const qreal dpr = devicePixelRatioF();
    // angleDelta is in 1/8 degree; a standard wheel notch (15 deg) scrolls
    // three text-ish lines worth of pixels.
    const QPointF delta = QPointF(event->angleDelta()) / 8.0 / 15.0 * 50.0 * dpr;
    applyEffects(syo_app_scroll_by(m_app, float(-delta.x()), float(-delta.y())));
}

void CanvasWidget::applyEffects(uint32_t effects)
{
    if (effects & SYO_EFFECT_QUIT) {
        emit quitRequested();
        return;
    }
    if (effects & SYO_EFFECT_OPEN_FILE_DIALOG)
        emit openFileRequested();
    if (effects & SYO_EFFECT_REDRAW)
        update();
    emit coreStateChanged();
}

QImage CanvasWidget::pageImage(size_t page)
{
    auto it = m_pageCache.find(page);
    if (it != m_pageCache.end())
        return it->image;

    SyoBitmap *bitmap = syo_app_render_page(m_app, page);
    if (!bitmap)
        return {};
    // Deep copy into a QImage the widget owns, then release the FFI buffer.
    QImage image(bitmap->data, int(bitmap->width), int(bitmap->height),
                 int(bitmap->width) * 4, QImage::Format_RGBA8888);
    QImage owned = image.copy();
    syo_bitmap_free(bitmap);

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

    if (!syo_app_has_document(m_app))
        return;

    const qreal dpr = devicePixelRatioF();

    SyoVisiblePage pages[64];
    const size_t count = syo_app_visible_pages(m_app, pages, 64);
    for (size_t i = 0; i < qMin<size_t>(count, 64); ++i) {
        const SyoVisiblePage &vp = pages[i];

        // Invalidate the cached image when the zoom changed: the bitmap the
        // core would render no longer matches the cached resolution.
        auto it = m_pageCache.find(vp.page);
        if (it != m_pageCache.end()
            && qAbs(qreal(it->image.width()) - qreal(vp.width)) > 1.5) {
            m_pageCache.erase(it);
        }

        const QImage image = pageImage(vp.page);
        if (image.isNull())
            continue;
        const QRectF target(vp.x / dpr, vp.y / dpr, vp.width / dpr, vp.height / dpr);
        painter.drawImage(target, image);
    }

    // Overlays. At most one is ever valid -- focus and visual are different
    // modes -- so collecting from both yields exactly one mode's rectangles.
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
    const auto addRect = [dpr](QPainterPath &path, float x, float y, float w, float h) {
        QRectF box(x / dpr, y / dpr, w / dpr, h / dpr);
        // Keep zero-size stops (spaces, empty lines) visible. Applied after the
        // dpr division, so the minimum is 2 logical pixels.
        if (box.width() < 2.0)
            box.setWidth(2.0);
        if (box.height() < 2.0)
            box.setHeight(2.0);
        path.addRect(box);
    };

    const auto fillOverlay = [&](SyoOverlay overlay, const QColor &color) {
        QPainterPath path;
        if (overlay.valid) {
            for (uintptr_t i = 0; i < overlay.rect_count; ++i) {
                const SyoRect r = overlay.rects[i];
                addRect(path, r.x, r.y, r.width, r.height);
            }
        }
        syo_overlay_free(overlay);
        if (!path.isEmpty())
            painter.fillPath(path.simplified(), color);
    };

    fillOverlay(syo_app_focus(m_app), m_focusColor);
    fillOverlay(syo_app_selection(m_app), m_visualColor);
}

} // namespace syodep
