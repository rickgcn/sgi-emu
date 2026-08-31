#pragma once

#include <QDockWidget>

namespace se_ui {

class DisassemblyDock final : public QDockWidget {
public:
    explicit DisassemblyDock(QWidget* parent = nullptr);
};

} // namespace se_ui
