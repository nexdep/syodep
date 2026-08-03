#include "sidebar/text_annotation_delegate.h"

#include <QPainter>
#include <QApplication>
#include <QFontMetrics>

#include "sidebar/text_annotation_list_model.h"

namespace syodep {

namespace {

QString elideLines(const QString &text, const QFontMetrics &fm, int width, int maxLines)
{
    const QString cleaned = QString(text).replace(QLatin1Char('\n'), QLatin1Char(' ')).trimmed();
    if (cleaned.isEmpty())
        return QObject::tr("(empty)");
    QString out;
    int lines = 0;
    int start = 0;
    while (start < cleaned.size() && lines < maxLines) {
        const QString remaining = cleaned.mid(start);
        const QString line = fm.elidedText(remaining, Qt::ElideRight, width);
        if (!out.isEmpty())
            out += QLatin1Char('\n');
        out += line;
        if (line.endsWith(QChar(0x2026)) || line.size() >= remaining.size())
            break;
        start += line.size();
        ++lines;
    }
    return out;
}

} // namespace

TextAnnotationDelegate::TextAnnotationDelegate(QObject *parent)
    : QStyledItemDelegate(parent)
{
}

void TextAnnotationDelegate::paint(QPainter *painter, const QStyleOptionViewItem &option,
                                     const QModelIndex &index) const
{
    painter->save();
    QStyleOptionViewItem opt(option);
    initStyleOption(&opt, index);

    const QStyle *style = opt.widget ? opt.widget->style() : QApplication::style();
    style->drawPrimitive(QStyle::PE_PanelItemViewItem, &opt, painter, opt.widget);

    const QRect pad = opt.rect.adjusted(10, 8, -10, -8);
    QFont pageFont = opt.font;
    pageFont.setPointSizeF(qMax(8.0, pageFont.pointSizeF() - 1.0));
    QFontMetrics pageFm(pageFont);

    const qsizetype first = index.data(TextAnnotationListModel::FirstPageRole).toLongLong();
    const qsizetype last = index.data(TextAnnotationListModel::LastPageRole).toLongLong();
    const QString pageText = first == last
        ? QObject::tr("Page %1").arg(first + 1)
        : QObject::tr("Pages %1–%2").arg(first + 1).arg(last + 1);

    painter->setFont(pageFont);
    painter->setPen(opt.palette.color(QPalette::PlaceholderText));
    painter->drawText(pad, Qt::AlignTop | Qt::AlignLeft, pageText);

    int y = pad.top() + pageFm.height() + 4;
    QFont quoteFont = opt.font;
    QFontMetrics quoteFm(quoteFont);
    const QString source = index.data(TextAnnotationListModel::TextRole).toString();
    const QString quote = elideLines(source, quoteFm, pad.width(), 2);
    painter->setFont(quoteFont);
    painter->setPen(opt.palette.color(QPalette::WindowText));
    const QRect quoteRect(pad.left(), y, pad.width(), quoteFm.lineSpacing() * 2);
    painter->drawText(quoteRect, Qt::TextWordWrap, quote);

    y += quoteFm.lineSpacing() * 2 + 4;
    QFont bodyFont = opt.font;
    bodyFont.setPointSizeF(qMax(8.0, bodyFont.pointSizeF() - 0.5));
    QFontMetrics bodyFm(bodyFont);
    const QString body = index.data(TextAnnotationListModel::BodyMarkdownRole).toString();
    // Plain-text card preview only — never treated as canonical Markdown.
    const QString preview = elideLines(body, bodyFm, pad.width(), 2);
    painter->setFont(bodyFont);
    painter->setPen(opt.palette.color(QPalette::PlaceholderText));
    painter->drawText(QRect(pad.left(), y, pad.width(), bodyFm.lineSpacing() * 2),
                      Qt::TextWordWrap, preview);
    painter->restore();
}

QSize TextAnnotationDelegate::sizeHint(const QStyleOptionViewItem &option,
                                         const QModelIndex &index) const
{
    Q_UNUSED(index);
    const int height = QFontMetrics(option.font).lineSpacing() * 7 + 24;
    return QSize(option.rect.width(), height);
}

} // namespace syodep
