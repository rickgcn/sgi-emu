#pragma once

#include "se_ui/settings_dialog.h"

#include <QMainWindow>

class QAction;
class QDockWidget;

namespace se_ui {

struct UiExitState;
struct UiStartupState;

class CacheDock;
class DisassemblyDock;
class MemoryDock;
class RegistersDock;
class TlbDock;

class MainWindow final : public QMainWindow {
public:
    explicit MainWindow(const UiStartupState& startup);

    [[nodiscard]] UiExitState exit_state() const;

private:
    void create_actions();
    void create_docks();
    void create_menus();
    void create_toolbar();
    void restore_window_state(const UiStartupState& startup);
    void set_default_dock_layout();
    void show_settings();

    MachineSettings settings_;

    QAction* run_action_;
    QAction* reset_action_;
    QAction* pause_action_;
    QAction* step_action_;
    QAction* settings_action_;

    DisassemblyDock* disassembly_dock_;
    RegistersDock* registers_dock_;
    TlbDock* tlb_dock_;
    CacheDock* cache_dock_;
    MemoryDock* memory_dock_;
};

UiExitState run_gui(const UiStartupState& startup);

} // namespace se_ui
