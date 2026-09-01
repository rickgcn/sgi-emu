#pragma once

#include <QDockWidget>

#include <cstdint>
#include <vector>

class QLineEdit;
class QPlainTextEdit;
class QCheckBox;
class QPushButton;

namespace se_ui {

struct UiSession;

class DisassemblyDock final : public QDockWidget {
public:
    DisassemblyDock(const UiSession& session, QWidget* parent = nullptr);

    void refresh();
    void clear();

private:
    void apply_address();
    void toggle_selected_breakpoint();

    const UiSession& session_;
    QLineEdit* address_edit_;
    QPushButton* breakpoint_button_;
    QCheckBox* follow_pc_;
    QPlainTextEdit* text_view_;
    std::uint32_t start_;
    std::uint64_t revision_;
    std::vector<std::uint32_t> line_addresses_;
};

} // namespace se_ui
