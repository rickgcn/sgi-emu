#include "se_ui/debugger/memory_dock.h"

#include <QLabel>

namespace se_ui {

MemoryDock::MemoryDock(QWidget* parent)
    : QDockWidget(QStringLiteral("Memory"), parent) {
    setObjectName(QStringLiteral("MemoryDock"));
    auto* label = new QLabel(QStringLiteral("No machine configured."), this);
    label->setAlignment(Qt::AlignCenter);
    setWidget(label);
}

} // namespace se_ui
