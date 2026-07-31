// Restricted Markdown preview: no HTML, no external resources, no auto-links.
#pragma once

#include <QTextBrowser>

namespace syodep {

class SafeMarkdownView final : public QTextBrowser
{
    Q_OBJECT

public:
    explicit SafeMarkdownView(QWidget *parent = nullptr);

    void setMarkdown(const QString &markdown);

protected:
    QVariant loadResource(int type, const QUrl &name) override;
};

} // namespace syodep
