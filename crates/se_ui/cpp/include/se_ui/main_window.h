#pragma once

#include "se_ui/settings_dialog.h"

#include <QMainWindow>

#include <memory>

class QAction;
class QDockWidget;
class QLabel;
class QTimer;

namespace se_ui {

struct UiExitState;
struct UiStartupState;
struct RuntimeStatusDto;
struct UiSession;

class CacheDock;
class DisassemblyDock;
class MemoryDock;
class RegistersDock;
class MachineOutputSink;
class SerialConsoleDock;
class TlbDock;

class MainWindow final : public QMainWindow {
public:
    MainWindow(const UiSession& session, const UiStartupState& startup);
    ~MainWindow() override;

    [[nodiscard]] UiExitState exit_state() const;

private:
    void create_actions();
    void create_docks();
    void create_menus();
    void create_status_bar();
    void create_toolbar();
    void restore_window_state(const UiStartupState& startup);
    void set_default_dock_layout();
    void show_settings();
    void update_runtime();
    void refresh_debuggers();
    void apply_runtime_status(const RuntimeStatusDto& status, bool report_error);
    void update_machine_status();

    const UiSession& session_;
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
    SerialConsoleDock* serial_console_dock_;
    std::shared_ptr<MachineOutputSink> machine_output_sink_;
    QTimer* update_timer_;
    QLabel* machine_status_;
    QLabel* execution_error_status_;
    QLabel* runtime_status_;
};

UiExitState run_gui(const UiSession& session, const UiStartupState& startup);

} // namespace se_ui
