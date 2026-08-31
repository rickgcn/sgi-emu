#pragma once

#include <QWidget>

namespace se_ui {

class DisplayWidget final : public QWidget {
public:
    explicit DisplayWidget(QWidget* parent = nullptr);
};

} // namespace se_ui
