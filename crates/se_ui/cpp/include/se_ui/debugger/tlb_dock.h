#pragma once

#include <QDockWidget>

#include <cstdint>

class QCheckBox;
class QLabel;
class QTabBar;
class QTableView;

namespace se_ui {

struct UiSession;

class TlbDock final : public QDockWidget {
public:
    TlbDock(const UiSession& session, QWidget* parent = nullptr);

    void refresh();
    void clear();

private:
    const UiSession& session_;
    QTabBar* tabs_;
    QCheckBox* valid_only_;
    QLabel* status_;
    QWidget* summary_;
    QLabel* shutdown_value_;
    QLabel* index_value_;
    QLabel* random_value_;
    QTableView* table_;
    std::uint64_t revision_;
};

} // namespace se_ui
