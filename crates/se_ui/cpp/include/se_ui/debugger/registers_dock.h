#pragma once

#include <QDockWidget>

namespace se_ui {

class RegistersDock final : public QDockWidget {
public:
    explicit RegistersDock(QWidget* parent = nullptr);
};

} // namespace se_ui
