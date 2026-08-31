#include "se_ui/debugger/cache_dock.h"

#include <QLabel>

namespace se_ui {

CacheDock::CacheDock(QWidget* parent)
    : QDockWidget(QStringLiteral("Cache"), parent) {
    setObjectName(QStringLiteral("CacheDock"));
    auto* label = new QLabel(QStringLiteral("No machine configured."), this);
    label->setAlignment(Qt::AlignCenter);
    setWidget(label);
}

} // namespace se_ui
