#pragma once

#include <QDockWidget>

namespace se_ui {

class MemoryDock final : public QDockWidget {
public:
    explicit MemoryDock(QWidget* parent = nullptr);
};

} // namespace se_ui
