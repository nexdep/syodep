#include "sidebar/highlight_delegate.h"

#include <QApplication>
#include <QPainter>
#include <QStyleOption>
#include <QTextLayout>
#include <QtMath>

#include "sidebar/highlight_list_model.h"

namespace syodep {

namespace {

int layoutWrappedText(const QString &text,
                      const QFont &font,
                      int width,
                      int maxLines,
                      qreal *outHeight)
{
    if (text.isEmpty() || width <= 0) {
        if (outHeight)
            *outHeight = 0;
        return 0;
    }
    QTextLayout textLayout(text, font);
    QTextOption textOpt;
    textOpt.setWrapMode(QTextOption::WrapAtWordBoundaryOrAnywhere);
    textLayout.setTextOption(textOpt);
    textLayout.beginLayout();
    qreal height = 0;
    int lines = 0;
    forever {
        QTextLine line = textLayout.createLine();
        if (!line.isValid())
            break;
        line.setLineWidth(width);
        line.setPosition(QPointF(0, height));
        height += line.height();
        ++lines;
        if (lines >= maxLines) {
            while (textLayout.createLine().isValid()) {
            }
            break;
        }
    }
    textLayout.endLayout();
    if (outHeight)
        *outHeight = height;
    return lines;
}

void paintWrappedText(QPainter *painter,
                      const QString &text,
                      const QFont &font,
                      const QRect &rect,
                      int maxLines,
                      const QColor &color)
{
    if (text.isEmpty() || !rect.isValid())
        return;
    painter->setPen(color);
    painter->setFont(font);

    QTextLayout textLayout(text, font);
    QTextOption textOpt;
    textOpt.setWrapMode(QTextOption::WrapAtWordBoundaryOrAnywhere);
    textLayout.setTextOption(textOpt);
    textLayout.beginLayout();

    qreal y = 0;
    int lines = 0;
    const QFontMetrics fm(font);
    forever {
        QTextLine line = textLayout.createLine();
        if (!line.isValid())
            break;
        line.setLineWidth(rect.width());
        if (lines + 1 >= maxLines) {
            const QString remaining = text.mid(line.textStart());
            const QString elided = fm.elidedText(remaining, Qt::ElideRight, rect.width());
            painter->drawText(
                QPointF(rect.left(), rect.top() + y + fm.ascent()),
                elided);
            while (textLayout.createLine().isValid()) {
            }
            break;
        }
        line.setPosition(QPointF(0, y));
        line.draw(painter, QPointF(rect.left(), rect.top()));
        y += line.height();
        ++lines;
    }
    textLayout.endLayout();
}

} // namespace

HighlightDelegate::HighlightDelegate(QObject *parent)
    : QStyledItemDelegate(parent)
{
}

HighlightDelegate::Layout HighlightDelegate::computeLayout(
    const QStyleOptionViewItem &option,
    const QModelIndex &index) const
{
    Layout layout;
    const QRect content = option.rect.adjusted(kMargin, kMargin, -kMargin, -kMargin);
    if (!content.isValid())
        return layout;

    const QFontMetrics metaFm(option.font);
    const int metaHeight = metaFm.height();

    layout.marker = QRect(content.left(),
                          content.top() + (metaHeight - kMarkerSize) / 2,
                          kMarkerSize,
                          kMarkerSize);

    const QString pageLabel = index.data(HighlightListModel::PageLabelRole).toString();
    const QString stateLabel = index.data(HighlightListModel::StateLabelRole).toString();
    const int pageWidth = metaFm.horizontalAdvance(pageLabel);

    layout.pageLabel = QRect(layout.marker.right() + kMetaGap,
                             content.top(),
                             pageWidth,
                             metaHeight);

    const int stateWidth = metaFm.horizontalAdvance(stateLabel);
    const int stateLeft = qMax(layout.pageLabel.right() + kMetaGap,
                               content.right() - stateWidth);
    layout.stateLabel = QRect(stateLeft, content.top(),
                              content.right() - stateLeft + 1,
                              metaHeight);

    const int textTop = content.top() + metaHeight + kTextGap;
    const int textWidth = content.width();
    const QString text = index.data(HighlightListModel::TextRole).toString();

    qreal textHeight = 0;
    layoutWrappedText(text, option.font, textWidth, kMaxTextLines, &textHeight);
    layout.text = QRect(content.left(), textTop, textWidth, int(qCeil(textHeight)));

    layout.height = layout.text.bottom() - option.rect.top() + kMargin;
    return layout;
}

void HighlightDelegate::paint(QPainter *painter,
                              const QStyleOptionViewItem &option,
                              const QModelIndex &index) const
{
    painter->save();

    QStyleOptionViewItem opt(option);
    initStyleOption(&opt, index);

    const QWidget *widget = option.widget;
    QStyle *style = widget ? widget->style() : QApplication::style();
    style->drawPrimitive(QStyle::PE_PanelItemViewItem, &opt, painter, widget);

    const Layout layout = computeLayout(opt, index);
    const QPalette &pal = opt.palette;
    const bool selected = opt.state & QStyle::State_Selected;

    QColor markerColor = index.data(HighlightListModel::ColorRole).value<QColor>();
    if (!markerColor.isValid())
        markerColor = pal.color(QPalette::Mid);
    painter->setRenderHint(QPainter::Antialiasing, true);
    painter->setPen(Qt::NoPen);
    painter->setBrush(markerColor);
    painter->drawEllipse(layout.marker);

    QColor metaColor = selected
        ? pal.color(QPalette::HighlightedText)
        : pal.color(QPalette::PlaceholderText);
    if (!metaColor.isValid() || metaColor.alpha() == 0)
        metaColor = pal.color(QPalette::Disabled, QPalette::WindowText);

    painter->setPen(metaColor);
    painter->setFont(opt.font);
    const QString pageLabel = index.data(HighlightListModel::PageLabelRole).toString();
    const QString stateLabel = index.data(HighlightListModel::StateLabelRole).toString();
    painter->drawText(layout.pageLabel, Qt::AlignLeft | Qt::AlignVCenter, pageLabel);

    QRect stateRect = layout.stateLabel;
    if (stateRect.left() < layout.pageLabel.right() + kMetaGap)
        stateRect.setLeft(layout.pageLabel.right() + kMetaGap);
    if (stateRect.width() > 0) {
        const QString elidedState = QFontMetrics(opt.font).elidedText(
            stateLabel, Qt::ElideRight, stateRect.width());
        painter->drawText(stateRect, Qt::AlignRight | Qt::AlignVCenter, elidedState);
    }

    const QString text = index.data(HighlightListModel::TextRole).toString();
    QColor textColor = pal.color(selected ? QPalette::HighlightedText : QPalette::Text);
    paintWrappedText(painter, text, opt.font, layout.text, kMaxTextLines, textColor);

    if (opt.state & QStyle::State_HasFocus) {
        QStyleOptionFocusRect focusOpt;
        focusOpt.QStyleOption::operator=(opt);
        focusOpt.rect = opt.rect;
        focusOpt.state |= QStyle::State_KeyboardFocusChange;
        focusOpt.backgroundColor = selected
            ? pal.color(QPalette::Highlight)
            : pal.color(QPalette::Window);
        style->drawPrimitive(QStyle::PE_FrameFocusRect, &focusOpt, painter, widget);
    }

    painter->restore();
}

QSize HighlightDelegate::sizeHint(const QStyleOptionViewItem &option,
                                  const QModelIndex &index) const
{
    QStyleOptionViewItem opt(option);
    if (opt.rect.width() <= 0 && option.widget)
        opt.rect.setWidth(option.widget->width());
    if (opt.rect.width() <= 0)
        opt.rect.setWidth(320);

    const Layout layout = computeLayout(opt, index);
    return QSize(opt.rect.width(),
                 qMax(layout.height, kMargin * 2 + QFontMetrics(opt.font).height()));
}

} // namespace syodep
