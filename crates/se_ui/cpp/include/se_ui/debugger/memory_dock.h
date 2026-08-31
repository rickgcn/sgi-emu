#pragma once

#include <QDockWidget>

#include <cstdint>

class QComboBox;
class QLineEdit;
class QPlainTextEdit;

namespace se_ui {

struct UiSession;

class MemoryDock final : public QDockWidget {
public:
    MemoryDock(const UiSession& session, QWidget* parent = nullptr);

    void refresh();
    void clear();

private:
    void apply_address();

    const UiSession& session_;
    QComboBox* address_space_;
    QLineEdit* address_edit_;
    QPlainTextEdit* text_view_;
    std::uint64_t start_;
    std::uint64_t revision_;
};

} // namespace se_ui
