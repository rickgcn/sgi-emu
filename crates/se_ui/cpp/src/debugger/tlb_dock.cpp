#include "se_ui/debugger/tlb_dock.h"

#include <QLabel>

namespace se_ui {

TlbDock::TlbDock(QWidget* parent)
    : QDockWidget(QStringLiteral("TLB"), parent) {
    setObjectName(QStringLiteral("TlbDock"));
    auto* label = new QLabel(QStringLiteral("No machine configured."), this);
    label->setAlignment(Qt::AlignCenter);
    setWidget(label);
}

} // namespace se_ui
