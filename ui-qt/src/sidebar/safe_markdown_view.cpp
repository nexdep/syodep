#include "sidebar/safe_markdown_view.h"

#include <QTextDocument>

namespace syodep {

SafeMarkdownView::SafeMarkdownView(QWidget *parent)
    : QTextBrowser(parent)
{
    setOpenExternalLinks(false);
    setOpenLinks(false);
    setReadOnly(true);
    setFrameShape(QFrame::NoFrame);
    setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    document()->setDefaultFont(font());
}

void SafeMarkdownView::setMarkdown(const QString &markdown)
{
    // MarkdownNoHTML rejects embedded HTML; resource loading is blocked below.
    document()->setMarkdown(
        markdown,
        QTextDocument::MarkdownFeatures(
            QTextDocument::MarkdownDialectGitHub | QTextDocument::MarkdownNoHTML));
}

QVariant SafeMarkdownView::loadResource(int type, const QUrl &name)
{
    Q_UNUSED(type);
    Q_UNUSED(name);
    // Block remote images, file URLs, and every other external resource.
    return QVariant();
}

} // namespace syodep
