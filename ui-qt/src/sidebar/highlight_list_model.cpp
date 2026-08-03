#include "sidebar/highlight_list_model.h"

#include <QColor>

namespace syodep {

QString highlightPageLabel(qsizetype firstPage, qsizetype lastPage)
{
    // Stored pages are zero-based; labels are one-based for the user.
    const qsizetype first = firstPage + 1;
    const qsizetype last = lastPage + 1;
    if (last <= first)
        return QObject::tr("Page %1").arg(first);
    return QObject::tr("Pages %1–%2").arg(first).arg(last);
}

QString highlightStateLabel(HighlightState state)
{
    switch (state) {
    case HighlightState::Pending:
        return QObject::tr("Pending PDF save");
    case HighlightState::Embedded:
        return QObject::tr("Embedded in PDF");
    case HighlightState::External:
        return QObject::tr("External annotation");
    }
    return QObject::tr("Pending PDF save");
}

HighlightListModel::HighlightListModel(QObject *parent)
    : QAbstractListModel(parent)
{
}

int HighlightListModel::rowCount(const QModelIndex &parent) const
{
    if (parent.isValid())
        return 0;
    return m_snapshot.items.size();
}

QVariant HighlightListModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid() || index.row() < 0 || index.row() >= m_snapshot.items.size())
        return {};

    const HighlightListItem &item = m_snapshot.items.at(index.row());
    switch (role) {
    case Qt::DisplayRole:
    case TextRole:
        return item.text;
    case Qt::ToolTipRole:
        return item.text;
    case Qt::AccessibleTextRole:
        return QStringLiteral("%1, %2. %3")
            .arg(highlightPageLabel(item.firstPage, item.lastPage),
                 highlightStateLabel(item.state),
                 item.text);
    case IdRole:
        return item.id;
    case ColorRole: {
        // Invalid strings stay invalid here; the delegate applies a palette
        // fallback for painting without writing a corrected value back.
        const QColor color(item.color);
        return color.isValid() ? QVariant::fromValue(color) : QVariant();
    }
    case FirstPageRole:
        return QVariant::fromValue(item.firstPage);
    case LastPageRole:
        return QVariant::fromValue(item.lastPage);
    case PageLabelRole:
        return highlightPageLabel(item.firstPage, item.lastPage);
    case StateRole:
        return QVariant::fromValue(static_cast<int>(item.state));
    case StateLabelRole:
        return highlightStateLabel(item.state);
    default:
        return {};
    }
}

QHash<int, QByteArray> HighlightListModel::roleNames() const
{
    return {
        {IdRole, "id"},
        {TextRole, "text"},
        {ColorRole, "color"},
        {FirstPageRole, "firstPage"},
        {LastPageRole, "lastPage"},
        {PageLabelRole, "pageLabel"},
        {StateRole, "state"},
        {StateLabelRole, "stateLabel"},
    };
}

void HighlightListModel::setSnapshot(HighlightSnapshot snapshot)
{
    beginResetModel();
    m_snapshot = std::move(snapshot);
    endResetModel();
}

const HighlightListItem *HighlightListModel::itemAt(int row) const
{
    if (row < 0 || row >= m_snapshot.items.size())
        return nullptr;
    return &m_snapshot.items.at(row);
}

std::optional<int> HighlightListModel::rowForId(qint64 id) const
{
    for (int row = 0; row < m_snapshot.items.size(); ++row) {
        if (m_snapshot.items.at(row).id == id)
            return row;
    }
    return std::nullopt;
}

} // namespace syodep
