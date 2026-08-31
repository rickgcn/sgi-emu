#pragma once

#include <QDockWidget>

namespace se_ui {

class TlbDock final : public QDockWidget {
public:
    explicit TlbDock(QWidget* parent = nullptr);
};

} // namespace se_ui
