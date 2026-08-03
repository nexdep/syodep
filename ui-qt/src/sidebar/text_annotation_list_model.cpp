#include "sidebar/text_annotation_list_model.h"

namespace syodep {

TextAnnotationListModel::TextAnnotationListModel(QObject *parent)
    : QAbstractListModel(parent)
{
}

int TextAnnotationListModel::rowCount(const QModelIndex &parent) const
{
    if (parent.isValid())
        return 0;
    return m_items.size();
}

QVariant TextAnnotationListModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid() || index.row() < 0 || index.row() >= m_items.size())
        return {};
    const TextAnnotationListItem &item = m_items.at(index.row());
    switch (role) {
    case Qt::DisplayRole:
    case TextRole:
        return item.text;
    case Qt::ToolTipRole: {
        QString tip = item.text;
        if (!item.bodyMarkdown.isEmpty()) {
            if (!tip.isEmpty())
                tip += QLatin1Char('\n');
            tip += item.bodyMarkdown;
        }
        return tip;
    }
    case IdRole:
        return item.id;
    case BodyMarkdownRole:
        return item.bodyMarkdown;
    case FirstPageRole:
        return item.firstPage;
    case LastPageRole:
        return item.lastPage;
    default:
        return {};
    }
}

QHash<int, QByteArray> TextAnnotationListModel::roleNames() const
{
    return {
        {IdRole, "id"},
        {TextRole, "text"},
        {BodyMarkdownRole, "bodyMarkdown"},
        {FirstPageRole, "firstPage"},
        {LastPageRole, "lastPage"},
    };
}

void TextAnnotationListModel::setSnapshot(const TextAnnotationSnapshot &snapshot)
{
    beginResetModel();
    m_items = snapshot.items;
    m_revision = snapshot.revision;
    endResetModel();
}

qint64 TextAnnotationListModel::idAt(int row) const
{
    if (row < 0 || row >= m_items.size())
        return 0;
    return m_items.at(row).id;
}

int TextAnnotationListModel::rowForId(qint64 id) const
{
    for (int i = 0; i < m_items.size(); ++i) {
        if (m_items.at(i).id == id)
            return i;
    }
    return -1;
}

TextAnnotationListItem TextAnnotationListModel::itemAt(int row) const
{
    if (row < 0 || row >= m_items.size())
        return {};
    return m_items.at(row);
}

} // namespace syodep
