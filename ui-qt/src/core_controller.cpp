#include "core_controller.h"

#include <QThread>
#include <QtGlobal>

#include <cstdio>

#include "syodep_ffi.h"

namespace syodep {

namespace {

void logUnexpectedHighlightState(int state)
{
    std::fprintf(stderr, "syodep: ignoring highlight with unknown pdf_state %d\n", state);
}

CoreOverlay takeOverlay(SyoOverlay overlay)
{
    CoreOverlay out;
    out.valid = overlay.valid != 0;
    if (overlay.valid && overlay.rects && overlay.rect_count > 0) {
        out.pixelRects.reserve(qsizetype(overlay.rect_count));
        for (size_t i = 0; i < overlay.rect_count; ++i) {
            const SyoRect &r = overlay.rects[i];
            out.pixelRects.push_back(QRectF(r.x, r.y, r.width, r.height));
        }
    }
    syo_overlay_free(overlay);
    return out;
}

} // namespace

CoreController::CoreController(QObject *parent)
    : CoreController(CorePersistence::Default, parent)
{
}

CoreController::CoreController(CorePersistence persistence, QObject *parent)
    : QObject(parent)
{
    assertGuiThread();

    const char *configPtr = nullptr;
    const char *dbPtr = nullptr;
    QByteArray configUtf8;
    QByteArray dbUtf8;

    if (persistence == CorePersistence::Default) {
        const QString configPath = takeSyoString(syo_default_config_path());
        const QString dbPath = takeSyoString(syo_default_db_path());
        configUtf8 = configPath.toUtf8();
        dbUtf8 = dbPath.toUtf8();
        configPtr = configUtf8.constData();
        dbPtr = dbUtf8.constData();
    }

    m_app = syo_app_new(configPtr, dbPtr);

    m_pendingInputTimer.setSingleShot(true);
    if (m_app)
        m_pendingInputTimer.setInterval(int(syo_app_key_timeout_ms(m_app)));
    connect(&m_pendingInputTimer, &QTimer::timeout, this, &CoreController::handleKeyTimeout);
}

CoreController::CoreController(const QString &databasePath, QObject *parent)
    : QObject(parent)
{
    assertGuiThread();

    const QString configPath = takeSyoString(syo_default_config_path());
    const QByteArray configUtf8 = configPath.toUtf8();
    const QByteArray dbUtf8 = databasePath.toUtf8();
    m_app = syo_app_new(configUtf8.constData(), dbUtf8.constData());

    m_pendingInputTimer.setSingleShot(true);
    if (m_app)
        m_pendingInputTimer.setInterval(int(syo_app_key_timeout_ms(m_app)));
    connect(&m_pendingInputTimer, &QTimer::timeout, this, &CoreController::handleKeyTimeout);
}

CoreController::~CoreController()
{
    assertGuiThread();
    m_pendingInputTimer.stop();
    if (m_app) {
        syo_app_free(m_app);
        m_app = nullptr;
    }
}

void CoreController::assertGuiThread() const
{
    Q_ASSERT(thread() == QThread::currentThread());
}

bool CoreController::hasDocument() const
{
    assertGuiThread();
    if (!m_app)
        return false;
    return syo_app_has_document(m_app);
}

bool CoreController::openDocument(const QString &path)
{
    assertGuiThread();
    if (!m_app)
        return false;

    const bool ok = syo_app_open_document(m_app, path.toUtf8().constData());
    // Match previous MainWindow behaviour: always invalidate the canvas cache
    // and refresh status, even when open fails (harmless; core keeps the prior
    // document on failure).
    emit pageCacheInvalidationRequested();
    if (ok) {
        emit documentChanged();
        emit annotationsChanged();
    }
    emit redrawRequested();
    emit statusChanged();
    return ok;
}

void CoreController::sendKey(const QString &chord)
{
    assertGuiThread();
    if (!m_app || chord.isEmpty())
        return;
    applyEffects(syo_app_key_event(m_app, chord.toUtf8().constData()));
}

void CoreController::handleKeyTimeout()
{
    assertGuiThread();
    if (!m_app)
        return;
    applyEffects(syo_app_key_timeout(m_app));
}

void CoreController::scrollBy(float dx, float dy)
{
    assertGuiThread();
    if (!m_app)
        return;
    applyEffects(syo_app_scroll_by(m_app, dx, dy));
}

void CoreController::setViewportSize(float width, float height)
{
    assertGuiThread();
    if (!m_app)
        return;
    syo_app_set_viewport(m_app, width, height);
    emit statusChanged();
}

void CoreController::setDevicePixelRatio(float ratio)
{
    assertGuiThread();
    if (!m_app)
        return;
    syo_app_set_device_pixel_ratio(m_app, ratio);
}

QVector<CoreVisiblePage> CoreController::visiblePages() const
{
    assertGuiThread();
    QVector<CoreVisiblePage> out;
    if (!m_app || !syo_app_has_document(m_app))
        return out;

    // The first call reports the true count even when it exceeds the stack
    // buffer — at minimum zoom a tall viewport can show more than 64 pages.
    SyoVisiblePage stack[64];
    size_t count = syo_app_visible_pages(m_app, stack, 64);
    if (count <= 64) {
        out.reserve(qsizetype(count));
        for (size_t i = 0; i < count; ++i) {
            CoreVisiblePage page;
            page.page = stack[i].page;
            page.pixelRect = QRectF(stack[i].x, stack[i].y, stack[i].width, stack[i].height);
            out.push_back(page);
        }
        return out;
    }

    QVector<SyoVisiblePage> heap{qsizetype(count)};
    count = syo_app_visible_pages(m_app, heap.data(), size_t(heap.size()));
    out.reserve(qsizetype(count));
    for (size_t i = 0; i < count; ++i) {
        CoreVisiblePage page;
        page.page = heap[qsizetype(i)].page;
        page.pixelRect = QRectF(heap[qsizetype(i)].x, heap[qsizetype(i)].y,
                                heap[qsizetype(i)].width, heap[qsizetype(i)].height);
        out.push_back(page);
    }
    return out;
}

QImage CoreController::renderPage(size_t page)
{
    assertGuiThread();
    if (!m_app)
        return {};

    SyoBitmap *bitmap = syo_app_render_page(m_app, page);
    if (!bitmap)
        return {};

    QImage image(bitmap->data, int(bitmap->width), int(bitmap->height),
                 int(bitmap->width) * 4, QImage::Format_RGBA8888);
    QImage owned = image.copy();
    syo_bitmap_free(bitmap);
    return owned;
}

CoreOverlay CoreController::focusOverlay() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeOverlay(syo_app_focus(m_app));
}

CoreOverlay CoreController::selectionOverlay() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeOverlay(syo_app_selection(m_app));
}

CoreOverlay CoreController::highlightOverlay() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeOverlay(syo_app_highlights(m_app));
}

QVector<CoreHighlightOverlay> CoreController::highlightOverlays() const
{
    assertGuiThread();
    QVector<CoreHighlightOverlay> out;
    if (!m_app)
        return out;
    SyoHighlightOverlayList *list = syo_app_highlight_overlays(m_app);
    if (!list)
        return out;
    out.reserve(qsizetype(list->count));
    for (size_t i = 0; i < list->count; ++i) {
        const SyoHighlightOverlay &item = list->items[i];
        CoreHighlightOverlay group;
        group.color = toQColor(item.color.r, item.color.g, item.color.b, item.color.a);
        group.pixelRects.reserve(qsizetype(item.rect_count));
        for (size_t j = 0; j < item.rect_count; ++j) {
            const SyoRect &r = item.rects[j];
            group.pixelRects.push_back(QRectF(r.x, r.y, r.width, r.height));
        }
        out.push_back(group);
    }
    syo_highlight_overlay_list_free(list);
    return out;
}

QColor CoreController::backgroundColor() const
{
    assertGuiThread();
    if (!m_app) {
        return QColor(QStringLiteral("#1e1e1e"));
    }
    const SyoColor c = syo_app_background_color(m_app);
    return toQColor(c.r, c.g, c.b, c.a);
}

QColor CoreController::focusColor() const
{
    assertGuiThread();
    if (!m_app)
        return QColor(0xad, 0xd8, 0xe6, 102);
    const SyoColor c = syo_app_focus_color(m_app);
    return toQColor(c.r, c.g, c.b, c.a);
}

QColor CoreController::visualColor() const
{
    assertGuiThread();
    if (!m_app)
        return QColor(0xd3, 0xd3, 0xd3, 102);
    const SyoColor c = syo_app_visual_color(m_app);
    return toQColor(c.r, c.g, c.b, c.a);
}

QColor CoreController::highlightColor() const
{
    assertGuiThread();
    if (!m_app)
        return QColor(0xff, 0xd4, 0x00, 102);
    const SyoColor c = syo_app_highlight_color(m_app);
    return toQColor(c.r, c.g, c.b, c.a);
}

QString CoreController::statusText() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeSyoString(syo_app_status_text(m_app));
}

QString CoreController::startupWarnings() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeSyoString(syo_app_startup_warnings(m_app));
}

QString CoreController::openDirectory() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeSyoString(syo_app_open_dir(m_app));
}

bool CoreController::startFullscreen() const
{
    assertGuiThread();
    if (!m_app)
        return true;
    return syo_app_start_fullscreen(m_app);
}

bool CoreController::startSidebarOpen() const
{
    assertGuiThread();
    if (!m_app)
        return false;
    return syo_app_start_sidebar_open(m_app);
}

QString CoreController::documentPath() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeSyoString(syo_app_document_path(m_app));
}

quint64 CoreController::annotationRevision() const
{
    assertGuiThread();
    if (!m_app)
        return 0;
    return quint64(syo_app_annotation_revision(m_app));
}

HighlightSnapshot CoreController::highlightSnapshot() const
{
    assertGuiThread();
    HighlightSnapshot snapshot;
    if (!m_app)
        return snapshot;

    SyoHighlightList *list = syo_app_highlight_list(m_app);
    if (!list)
        return snapshot;

    snapshot.revision = quint64(list->revision);
    snapshot.items.reserve(qsizetype(list->count));
    for (size_t i = 0; i < list->count; ++i) {
        const SyoHighlightItem &item = list->items[i];
        bool ok = false;
        const HighlightState state = highlightStateFromFfi(item.state, &ok);
        if (!ok) {
            logUnexpectedHighlightState(item.state);
            continue;
        }
        HighlightListItem out;
        out.id = item.id;
        out.text = item.text ? QString::fromUtf8(item.text) : QString();
        out.color = item.color ? QString::fromUtf8(item.color) : QString();
        out.firstPage = qsizetype(item.first_page);
        out.lastPage = qsizetype(item.last_page);
        out.state = state;
        snapshot.items.push_back(out);
    }
    syo_highlight_list_free(list);
    return snapshot;
}

void CoreController::revealHighlight(qint64 id)
{
    assertGuiThread();
    if (!m_app)
        return;
    applyEffects(syo_app_reveal_highlight(m_app, id));
}

QString CoreController::highlightMarkdown(qint64 id) const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeSyoString(syo_app_highlight_markdown(m_app, id));
}

QString CoreController::allHighlightsMarkdown() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeSyoString(syo_app_all_highlights_markdown(m_app));
}

bool CoreController::deleteHighlight(qint64 highlightId)
{
    assertGuiThread();
    if (!m_app)
        return false;

    uint32_t effects = 0;
    const bool ok = syo_app_delete_highlight(m_app, highlightId, &effects);
    if (ok)
        applyEffects(effects);
    else
        emit statusChanged(); // the refusal is in the core's status text
    return ok;
}

TextAnnotationSnapshot CoreController::textAnnotationSnapshot() const
{
    assertGuiThread();
    TextAnnotationSnapshot snapshot;
    if (!m_app)
        return snapshot;

    SyoTextAnnotationList *list = syo_app_text_annotation_list(m_app);
    if (!list)
        return snapshot;

    snapshot.revision = quint64(list->revision);
    snapshot.items.reserve(qsizetype(list->count));
    for (size_t i = 0; i < list->count; ++i) {
        const SyoTextAnnotationItem &item = list->items[i];
        TextAnnotationListItem out;
        out.id = item.id;
        out.text = item.text ? QString::fromUtf8(item.text) : QString();
        out.bodyMarkdown =
            item.body_markdown ? QString::fromUtf8(item.body_markdown) : QString();
        out.firstPage = qsizetype(item.first_page);
        out.lastPage = qsizetype(item.last_page);
        snapshot.items.push_back(out);
    }
    syo_text_annotation_list_free(list);
    return snapshot;
}

QString CoreController::pendingAnnotationText() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeSyoString(syo_app_pending_annotation_text(m_app));
}

void CoreController::revealTextAnnotation(qint64 id)
{
    assertGuiThread();
    if (!m_app)
        return;
    applyEffects(syo_app_reveal_text_annotation(m_app, id));
}

QString CoreController::textAnnotationMarkdown(qint64 id) const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeSyoString(syo_app_text_annotation_markdown(m_app, id));
}

QString CoreController::allTextAnnotationsMarkdown() const
{
    assertGuiThread();
    if (!m_app)
        return {};
    return takeSyoString(syo_app_all_text_annotations_markdown(m_app));
}

bool CoreController::deleteTextAnnotation(qint64 annotationId)
{
    assertGuiThread();
    if (!m_app)
        return false;

    uint32_t effects = 0;
    const bool ok = syo_app_delete_text_annotation(m_app, annotationId, &effects);
    if (ok)
        applyEffects(effects);
    else
        emit statusChanged();
    return ok;
}

bool CoreController::createTextAnnotation(const QString &bodyMarkdown, qint64 *createdId)
{
    assertGuiThread();
    if (createdId)
        *createdId = 0;
    if (!m_app)
        return false;

    uint32_t effects = 0;
    int64_t id = 0;
    const QByteArray utf8 = bodyMarkdown.toUtf8();
    const bool ok =
        syo_app_create_text_annotation(m_app, utf8.constData(), &id, &effects);
    if (ok) {
        if (createdId)
            *createdId = static_cast<qint64>(id);
        applyEffects(effects);
    } else {
        emit statusChanged();
    }
    return ok;
}

bool CoreController::hasPersistence() const
{
    assertGuiThread();
    if (!m_app)
        return false;
    return syo_app_has_persistence(m_app);
}

bool CoreController::setTextAnnotationBody(qint64 annotationId,
                                           const QString &bodyMarkdown)
{
    assertGuiThread();
    if (!m_app)
        return false;

    uint32_t effects = 0;
    const QByteArray utf8 = bodyMarkdown.toUtf8();
    const bool ok = syo_app_set_text_annotation_body(
        m_app, annotationId, utf8.constData(), &effects);
    if (ok)
        applyEffects(effects);
    else
        emit statusChanged();
    return ok;
}

void CoreController::cancelPendingAnnotation()
{
    assertGuiThread();
    if (!m_app)
        return;
    syo_app_cancel_pending_annotation(m_app);
}

bool CoreController::hasUnsavedHighlights() const
{
    assertGuiThread();
    if (!m_app)
        return false;
    return syo_app_has_unsaved_highlights(m_app);
}

bool CoreController::quitSaving()
{
    assertGuiThread();
    if (!m_app)
        return false;
    return applyEffects(syo_app_quit_save(m_app), QuitDelivery::ReturnOnly);
}

bool CoreController::quitDiscarding()
{
    assertGuiThread();
    if (!m_app)
        return false;
    return applyEffects(syo_app_quit_discard(m_app), QuitDelivery::ReturnOnly);
}

bool CoreController::applyEffects(uint32_t effects, QuitDelivery quitDelivery)
{
    assertGuiThread();

    if (effects & SYO_EFFECT_QUIT) {
        m_pendingInputTimer.stop();
        if (quitDelivery == QuitDelivery::EmitSignal)
            emit quitRequested();
        return true;
    }

    // 1. Pending-key timer
    if ((effects & SYO_EFFECT_PENDING_INPUT) && m_pendingInputTimer.interval() > 0)
        m_pendingInputTimer.start();
    else
        m_pendingInputTimer.stop();

    // 2. Cache invalidation before redraw
    if (effects & SYO_EFFECT_RELOAD)
        emit pageCacheInvalidationRequested();

    // 3. Annotation list refresh
    if (effects & SYO_EFFECT_ANNOTATIONS_CHANGED)
        emit annotationsChanged();

    // 4. Open-file dialog
    if (effects & SYO_EFFECT_OPEN_FILE_DIALOG)
        emit openFileRequested();

    // 4b. Sidebar visibility. Before the redraw so the canvas resizes once.
    if (effects & SYO_EFFECT_TOGGLE_HIGHLIGHTS_SIDEBAR)
        emit toggleHighlightsSidebarRequested();
    if (effects & SYO_EFFECT_TOGGLE_ANNOTATIONS_SIDEBAR)
        emit toggleAnnotationsSidebarRequested();
    if (effects & SYO_EFFECT_CREATE_ANNOTATION_REQUESTED)
        emit createTextAnnotationRequested();

    // 5. Canvas redraw
    if (effects & SYO_EFFECT_REDRAW)
        emit redrawRequested();

    // 6. Status line (mode, pending keys, errors) after every handled input
    emit statusChanged();

    // 7. Confirm quit last so the modal sees updated status/state first
    if (effects & SYO_EFFECT_CONFIRM_QUIT)
        emit confirmQuitRequested();

    return false;
}

QString CoreController::takeSyoString(char *value)
{
    if (!value)
        return {};
    const QString out = QString::fromUtf8(value);
    syo_string_free(value);
    return out;
}

QColor CoreController::toQColor(uint8_t r, uint8_t g, uint8_t b, uint8_t a)
{
    return QColor(r, g, b, a);
}

HighlightState CoreController::highlightStateFromFfi(int state, bool *ok)
{
    switch (state) {
    case SYO_HIGHLIGHT_PENDING:
        if (ok)
            *ok = true;
        return HighlightState::Pending;
    case SYO_HIGHLIGHT_EMBEDDED:
        if (ok)
            *ok = true;
        return HighlightState::Embedded;
    case SYO_HIGHLIGHT_EXTERNAL:
        if (ok)
            *ok = true;
        return HighlightState::External;
    default:
        if (ok)
            *ok = false;
        return HighlightState::Pending;
    }
}

} // namespace syodep
