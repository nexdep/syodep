// List model for Markdown text annotations. Document order comes from Rust.
#pragma once

#include <QAbstractListModel>
#include <QVector>

#include "core_controller.h"

namespace syodep {

class TextAnnotationListModel final : public QAbstractListModel
{
    Q_OBJECT

public:
    enum Roles {
        IdRole = Qt::UserRole + 1,
        TextRole,
        BodyMarkdownRole,
        FirstPageRole,
        LastPageRole
    };

    explicit TextAnnotationListModel(QObject *parent = nullptr);

    int rowCount(const QModelIndex &parent = QModelIndex()) const override;
    QVariant data(const QModelIndex &index, int role = Qt::DisplayRole) const override;
    QHash<int, QByteArray> roleNames() const override;

    void setSnapshot(const TextAnnotationSnapshot &snapshot);
    quint64 revision() const { return m_revision; }
    qint64 idAt(int row) const;
    int rowForId(qint64 id) const;
    TextAnnotationListItem itemAt(int row) const;

private:
    QVector<TextAnnotationListItem> m_items;
    quint64 m_revision = 0;
};

} // namespace syodep
