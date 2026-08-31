#include "se_ui/display_widget.h"

namespace se_ui {

DisplayWidget::DisplayWidget(QWidget* parent)
    : QWidget(parent) {
    setObjectName(QStringLiteral("DisplayWidget"));
}

} // namespace se_ui
