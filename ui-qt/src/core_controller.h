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
    QString noteMarkdown;
    bool hasNote = false;
    qsizetype firstPage = 0;
    qsizetype lastPage = 0;
    HighlightState state = HighlightState::Pending;
};

struct HighlightSnapshot
{
    QVector<HighlightListItem> items;
    quint64 revision = 0;
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
    void scrollBy(float dx, float dy);
    void setViewportSize(float width, float height);

    // Rendering queries (owned Qt values; no FFI pointers escape)
    QVector<CoreVisiblePage> visiblePages() const;
    QImage renderPage(size_t page);
    CoreOverlay focusOverlay() const;
    CoreOverlay selectionOverlay() const;
    CoreOverlay highlightOverlay() const;

    // Appearance and status
    QColor backgroundColor() const;
    QColor focusColor() const;
    QColor visualColor() const;
    QColor highlightColor() const;

    QString statusText() const;
    QString startupWarnings() const;
    QString openDirectory() const;

    // Annotation queries (Pending + Embedded; not filtered like the overlay)
    quint64 annotationRevision() const;
    HighlightSnapshot highlightSnapshot() const;

    // Annotation actions — Markdown and navigation stay core-owned
    void revealHighlight(qint64 id);
    QString highlightMarkdown(qint64 id) const;
    QString allHighlightsMarkdown() const;
    // Returns true only when the core confirms the save. Whitespace-only bodies
    // clear the comment (core-owned rule). Does not trim or rewrite Markdown.
    bool setHighlightNote(qint64 highlightId, const QString &bodyMarkdown);

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

private:
    enum class QuitDelivery
    {
        EmitSignal,
        ReturnOnly
    };

    void assertGuiThread() const;
    bool applyEffects(uint32_t effects,
                      QuitDelivery quitDelivery = QuitDelivery::EmitSignal);

    static QString takeSyoString(char *value);
    static QColor toQColor(uint8_t r, uint8_t g, uint8_t b, uint8_t a);
    static HighlightState highlightStateFromFfi(int state, bool *ok);

    SyoApp *m_app = nullptr;
    QTimer m_pendingInputTimer;
};

} // namespace syodep
