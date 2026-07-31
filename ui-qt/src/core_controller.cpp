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

QVector<CoreVisiblePage> CoreController::visiblePages() const
{
    assertGuiThread();
    QVector<CoreVisiblePage> out;
    if (!m_app || !syo_app_has_document(m_app))
        return out;

    SyoVisiblePage pages[64];
    const size_t count = syo_app_visible_pages(m_app, pages, 64);
    out.reserve(qsizetype(qMin<size_t>(count, 64)));
    for (size_t i = 0; i < qMin<size_t>(count, 64); ++i) {
        CoreVisiblePage page;
        page.page = pages[i].page;
        page.pixelRect = QRectF(pages[i].x, pages[i].y, pages[i].width, pages[i].height);
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
        out.hasNote = item.has_note != 0;
        out.noteMarkdown = (item.has_note != 0 && item.note_markdown)
            ? QString::fromUtf8(item.note_markdown)
            : QString();
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

bool CoreController::setHighlightNote(qint64 highlightId, const QString &bodyMarkdown)
{
    assertGuiThread();
    if (!m_app)
        return false;

    const QByteArray utf8 = bodyMarkdown.toUtf8();
    uint32_t effects = 0;
    const bool ok = syo_app_set_highlight_note(
        m_app, highlightId, utf8.constData(), &effects);
    if (ok)
        applyEffects(effects);
    else
        emit statusChanged();
    return ok;
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
