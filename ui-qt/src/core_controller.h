// Mechanical Qt doorway into the Rust core.
//
// Owns the live-window SyoApp*, converts C ABI values into owned Qt types,
// routes effect bits to signals, and owns the pending-key QTimer. Contains no
// document, traversal, or annotation business logic — that stays in Rust.
//
// Threading: CoreController and its SyoApp* must remain on the GUI thread.
// Future agent workers must post queued events rather than calling in from
// background threads.
#pragma once

#include <QColor>
#include <QImage>
#include <QObject>
#include <QRectF>
#include <QString>
#include <QTimer>
#include <QVector>

#include <QtGlobal>

#include <cstdint>

struct SyoApp;

namespace syodep {

// Core/canvas pixel coordinates, before devicePixelRatio conversion.
struct CoreVisiblePage
{
    size_t page = 0;
    QRectF pixelRect;
};

// Core/canvas pixel coordinates for an overlay. Widgets must not free FFI
// memory; the controller copies and releases it.
struct CoreOverlay
{
    bool valid = false;
    QVector<QRectF> pixelRects;
};

// One colour group of pending-highlight rectangles.
struct CoreHighlightOverlay
{
    QColor color;
    QVector<QRectF> pixelRects;
};

enum class HighlightState
{
    Pending,
    Embedded,
    External
};

// Disposable presentation snapshot — not the source of truth.
struct HighlightListItem
{
    qint64 id = 0;
    QString text;
    QString color;
    qsizetype firstPage = 0;
    qsizetype lastPage = 0;
    HighlightState state = HighlightState::Pending;
};

struct HighlightSnapshot
{
    QVector<HighlightListItem> items;
    quint64 revision = 0;
};

// Disposable presentation snapshot for Markdown annotations — full body, not a preview.
struct TextAnnotationListItem
{
    qint64 id = 0;
    QString text;
    QString bodyMarkdown;
    qsizetype firstPage = 0;
    qsizetype lastPage = 0;
};

struct TextAnnotationSnapshot
{
    QVector<TextAnnotationListItem> items;
    quint64 revision = 0;
};

enum class KeybindingGroup
{
    Help,
    Common,
    Normal,
    Focus,
    Visual,
    Highlight
};

enum class DocumentMode
{
    Normal,
    Focus,
    Visual,
    Highlight
};

struct KeybindingItem
{
    KeybindingGroup group = KeybindingGroup::Common;
    QString keys;
    QString command;
    QString description;
};

struct KeybindingSnapshot
{
    QVector<KeybindingItem> items;
    DocumentMode activeMode = DocumentMode::Normal;
};

enum class HelpNavigation
{
    LineDown,
    LineUp,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    Top,
    Bottom
};

enum class CorePersistence
{
    // Resolve default config and database paths (normal window).
    Default,
    // Pass null paths to the FFI (smoke/CI: no user database).
    Disabled
};

class CoreController final : public QObject
{
    Q_OBJECT

public:
    explicit CoreController(QObject *parent = nullptr);
    explicit CoreController(CorePersistence persistence, QObject *parent = nullptr);
    // Smoke/tests: open with an explicit database path (config still defaults).
    explicit CoreController(const QString &databasePath, QObject *parent = nullptr);
    ~CoreController() override;

    CoreController(const CoreController &) = delete;
    CoreController &operator=(const CoreController &) = delete;

    bool isValid() const { return m_app != nullptr; }

    // Lifecycle and document state
    bool hasDocument() const;
    bool openDocument(const QString &path);

    // Input and view
    void sendKey(const QString &chord);
    void handleKeyTimeout();
    // Isolated input path used while keyboard focus is inside the annotation
    // sidebar. Returns whether the focused widget should consume the event.
    bool sendSidebarKey(const QString &chord);
    void handleSidebarKeyTimeout();
    void cancelSidebarInput();
    // Configured multi-key timeout, so shell-local key sequences (the
    // sidebar's `gg`/`dd`) expire on the same clock as the core's.
    int keyTimeoutMs() const { return m_pendingInputTimer.interval(); }
    void scrollBy(float dx, float dy);
    void setViewportSize(float width, float height);
    // Physical pixels per logical pixel (Qt devicePixelRatio). Configured
    // scroll distances are logical; the core multiplies them by this.
    void setDevicePixelRatio(float ratio);

    // Rendering queries (owned Qt values; no FFI pointers escape)
    QVector<CoreVisiblePage> visiblePages() const;
    QImage renderPage(size_t page);
    CoreOverlay focusOverlay() const;
    CoreOverlay selectionOverlay() const;
    // Legacy single-colour flatten; prefer highlightOverlays().
    CoreOverlay highlightOverlay() const;
    // Pending highlights grouped by the colour each one captured.
    QVector<CoreHighlightOverlay> highlightOverlays() const;

    // Appearance and status
    QColor backgroundColor() const;
    QColor focusColor() const;
    QColor visualColor() const;
    QColor highlightColor() const;

    QString statusText() const;
    QString startupWarnings() const;
    QString openDirectory() const;
    // Whether the shell should open the main window fullscreen ([window]
    // start_fullscreen).
    bool startFullscreen() const;
    // Whether the Highlights sidebar starts open ([window] start_sidebar_open).
    bool startSidebarOpen() const;
    // Empty when no document is open.
    QString documentPath() const;
    bool keybindingsOverlayVisible() const;
    KeybindingSnapshot keybindingSnapshot() const;

    // Annotation queries (Pending + Embedded; not filtered like the overlay)
    quint64 annotationRevision() const;
    HighlightSnapshot highlightSnapshot() const;
    TextAnnotationSnapshot textAnnotationSnapshot() const;
    QString pendingAnnotationText() const;

    // Annotation actions — Markdown and navigation stay core-owned
    void revealHighlight(qint64 id);
    QString highlightMarkdown(qint64 id) const;
    QString allHighlightsMarkdown() const;
    // Returns true only when the core confirms the deletion. Removing an
    // Embedded highlight rewrites the PDF, so the effects the core reports
    // (and this controller applies) may include a page-cache invalidation.
    // Failures leave everything in place and the reason in statusText().
    bool deleteHighlight(qint64 highlightId);

    void revealTextAnnotation(qint64 id);
    QString textAnnotationMarkdown(qint64 id) const;
    QString allTextAnnotationsMarkdown() const;
    bool deleteTextAnnotation(qint64 annotationId);
    // On success writes the new stable id to *createdId when non-null.
    bool createTextAnnotation(const QString &bodyMarkdown, qint64 *createdId = nullptr);
    bool setTextAnnotationBody(qint64 annotationId, const QString &bodyMarkdown);
    void cancelPendingAnnotation();
    bool hasPersistence() const;

    // Quit flow
    bool hasUnsavedHighlights() const;
    // Synchronous close-confirmation path: apply effects without emitting
    // quitRequested (avoids re-entrancy while closeEvent is deciding).
    bool quitSaving();
    bool quitDiscarding();

signals:
    void redrawRequested();
    void pageCacheInvalidationRequested();

    void documentChanged();
    void annotationsChanged();
    void statusChanged();

    void openFileRequested();
    void quitRequested();
    void confirmQuitRequested();
    // `<leader>a` reached the core. Sidebar visibility is shell state, so the
    // core only asks; MainWindow decides what "toggle" means, including where
    // keyboard focus lands.
    void toggleHighlightsSidebarRequested();
    void toggleAnnotationsSidebarRequested();
    void closeSidebarRequested();
    // `n` captured a pending anchor; open Annotations in creation mode.
    void createTextAnnotationRequested();
    void keybindingsOverlayChanged();
    void helpNavigationRequested(syodep::HelpNavigation navigation);

private:
    enum class QuitDelivery
    {
        EmitSignal,
        ReturnOnly
    };

    void assertGuiThread() const;
    bool applyEffects(uint32_t effects,
                      QuitDelivery quitDelivery = QuitDelivery::EmitSignal,
                      bool updateDocumentInputTimer = true);
    void updateSidebarInputTimer(bool pending);

    static QString takeSyoString(char *value);
    static QColor toQColor(uint8_t r, uint8_t g, uint8_t b, uint8_t a);
    static HighlightState highlightStateFromFfi(int state, bool *ok);

    SyoApp *m_app = nullptr;
    QTimer m_pendingInputTimer;
    QTimer m_sidebarInputTimer;
};

} // namespace syodep
