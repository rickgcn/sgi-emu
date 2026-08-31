#include "se_ui/debugger/disassembly_dock.h"

#include <QLabel>

namespace se_ui {

DisassemblyDock::DisassemblyDock(QWidget* parent)
    : QDockWidget(QStringLiteral("Disassembly"), parent) {
    setObjectName(QStringLiteral("DisassemblyDock"));
    auto* label = new QLabel(QStringLiteral("No machine configured."), this);
    label->setAlignment(Qt::AlignCenter);
    setWidget(label);
}

} // namespace se_ui
