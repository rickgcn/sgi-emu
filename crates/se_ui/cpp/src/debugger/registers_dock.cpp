#include "se_ui/debugger/registers_dock.h"

#include <QLabel>

namespace se_ui {

RegistersDock::RegistersDock(QWidget* parent)
    : QDockWidget(QStringLiteral("Registers"), parent) {
    setObjectName(QStringLiteral("RegistersDock"));
    auto* label = new QLabel(QStringLiteral("No machine configured."), this);
    label->setAlignment(Qt::AlignCenter);
    setWidget(label);
}

} // namespace se_ui
