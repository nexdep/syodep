// Qt projection of a disposable HighlightSnapshot from CoreController.
// Not the source of truth — full resets on revision change are intentional.
#pragma once

#include <QAbstractListModel>
#include <QHash>
#include <QVariant>
#include <QVector>

#include <optional>

#include "core_controller.h"

namespace syodep {

// One-based page label for display only. Does not mutate stored zero-based pages.
QString highlightPageLabel(qsizetype firstPage, qsizetype lastPage);

// User-facing persistence label for a highlight state.
QString highlightStateLabel(HighlightState state);

// Plain-text preview derived from Markdown for card display only.
QString markdownToPlainPreview(const QString &markdown);

class HighlightListModel final : public QAbstractListModel
{
    Q_OBJECT

public:
    enum Role {
        IdRole = Qt::UserRole + 1,
        TextRole,
        ColorRole,
        FirstPageRole,
        LastPageRole,
        PageLabelRole,
        StateRole,
        StateLabelRole,
        HasNoteRole,
        NoteMarkdownRole,
        NotePlainPreviewRole
    };

    explicit HighlightListModel(QObject *parent = nullptr);

    int rowCount(const QModelIndex &parent = QModelIndex()) const override;
    QVariant data(const QModelIndex &index, int role = Qt::DisplayRole) const override;
    QHash<int, QByteArray> roleNames() const override;

    void setSnapshot(HighlightSnapshot snapshot);

    quint64 revision() const { return m_snapshot.revision; }
    const HighlightListItem *itemAt(int row) const;
    std::optional<int> rowForId(qint64 id) const;

private:
    HighlightSnapshot m_snapshot;
};

} // namespace syodep
