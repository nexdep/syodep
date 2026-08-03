// Paints one highlight card in the annotation sidebar.
// No CoreController calls — presentation only.
#pragma once

#include <QStyledItemDelegate>

namespace syodep {

class HighlightDelegate final : public QStyledItemDelegate
{
    Q_OBJECT

public:
    explicit HighlightDelegate(QObject *parent = nullptr);

    void paint(QPainter *painter,
               const QStyleOptionViewItem &option,
               const QModelIndex &index) const override;

    QSize sizeHint(const QStyleOptionViewItem &option,
                   const QModelIndex &index) const override;

private:
    static constexpr int kMaxTextLines = 4;
    static constexpr int kMargin = 10;
    static constexpr int kMarkerSize = 10;
    static constexpr int kMetaGap = 8;
    static constexpr int kTextGap = 6;

    struct Layout
    {
        QRect marker;
        QRect pageLabel;
        QRect stateLabel;
        QRect text;
        int height = 0;
    };

    Layout computeLayout(const QStyleOptionViewItem &option,
                         const QModelIndex &index) const;
};

} // namespace syodep
