// Card delegate for text annotations: source quote + bounded body preview.
#pragma once

#include <QStyledItemDelegate>

namespace syodep {

class TextAnnotationDelegate final : public QStyledItemDelegate
{
    Q_OBJECT

public:
    explicit TextAnnotationDelegate(QObject *parent = nullptr);

    void paint(QPainter *painter, const QStyleOptionViewItem &option,
               const QModelIndex &index) const override;
    QSize sizeHint(const QStyleOptionViewItem &option,
                   const QModelIndex &index) const override;
};

} // namespace syodep
