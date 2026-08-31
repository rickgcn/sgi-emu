#pragma once

#include <QDockWidget>

class QCheckBox;
class QLabel;
class QPushButton;
class QTabBar;
class QTableView;

namespace se_ui {

struct UiSession;

class CacheDock final : public QDockWidget {
public:
    CacheDock(const UiSession& session, QWidget* parent = nullptr);

    void refresh();
    void clear();

private:
    const UiSession& session_;
    QTabBar* tabs_;
    QCheckBox* valid_only_;
    QLabel* status_;
    QPushButton* refresh_button_;
    QTableView* table_;
};

} // namespace se_ui
