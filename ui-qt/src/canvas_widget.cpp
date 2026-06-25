#include "canvas_widget.h"

#include <QPainter>
#include <QWheelEvent>

#include "key_encoder.h"

namespace syodep {

CanvasWidget::CanvasWidget(SyoApp *app, QWidget *parent)
    : QOpenGLWidget(parent)
    , m_app(app)
    , m_background(QStringLiteral("#1e1e1e"))
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

    // Caret overlay (only present in caret focus mode). The core returns its rect in
    // canvas pixels; we draw a translucent fill plus a solid border.
    const SyoCaret caret = syo_app_caret(m_app);
    if (caret.valid) {
        QRectF box(caret.x / dpr, caret.y / dpr, caret.width / dpr, caret.height / dpr);
        if (box.width() < 2.0)
            box.setWidth(2.0); // keep zero-width stops (e.g. spaces) visible
        const QColor accent(80, 160, 255);
        painter.fillRect(box, QColor(80, 160, 255, 70));
        painter.setPen(accent);
        painter.setBrush(Qt::NoBrush);
        painter.drawRect(box);
    }
}

} // namespace syodep
