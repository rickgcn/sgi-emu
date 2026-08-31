#pragma once

#include <QDockWidget>

namespace se_ui {

class CacheDock final : public QDockWidget {
public:
    explicit CacheDock(QWidget* parent = nullptr);
};

} // namespace se_ui
