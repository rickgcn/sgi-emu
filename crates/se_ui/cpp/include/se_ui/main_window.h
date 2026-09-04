#pragma once

#include "se_ui/settings_dialog.h"

#include <QElapsedTimer>
#include <QMainWindow>

#include <cstdint>
#include <functional>
#include <memory>
#include <string>

class QAction;
class QDockWidget;
class QLabel;
class QString;
class QTimer;

namespace se_ui {

struct UiExitState;
struct UiStartupState;
struct RuntimeStatusDto;
struct UiSession;

class CacheDock;
class DisassemblyDock;
class MemoryDock;
class PreparationTask;
class RegistersDock;
class MachineOutputSink;
class SerialConsoleDock;
class TlbDock;

enum class PreparationState {
    None,
    Recording,
    Replay,
};

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
    void show_notification(const QString& message, int timeout);
    void begin_preparation(
        PreparationState state,
        bool stops_replay,
        std::function<RuntimeStatusDto()> command);
    void poll_preparation();
    void apply_preparation_state();
    void run_with_record();
    void stop_recording();
    void open_replay();
    void stop_replay();
    void show_settings();
    void update_runtime();
    void refresh_debuggers();
    void apply_runtime_status(const RuntimeStatusDto& status, bool report_error);
    void update_performance_status(const RuntimeStatusDto& status);
    void update_machine_status();

    const UiSession& session_;
    MachineSettings settings_;

    QAction* run_action_;
    QAction* run_with_record_action_;
    QAction* reset_action_;
    QAction* pause_action_;
    QAction* step_action_;
    QAction* stop_recording_action_;
    QAction* open_replay_action_;
    QAction* stop_replay_action_;
    QAction* settings_action_;

    DisassemblyDock* disassembly_dock_;
    RegistersDock* registers_dock_;
    TlbDock* tlb_dock_;
    CacheDock* cache_dock_;
    MemoryDock* memory_dock_;
    SerialConsoleDock* serial_console_dock_;
    std::shared_ptr<MachineOutputSink> machine_output_sink_;
    QTimer* update_timer_;
    QTimer* notification_timer_;
    QElapsedTimer performance_timer_;
    std::unique_ptr<PreparationTask> preparation_task_;
    PreparationState preparation_state_;
    bool preparation_resume_running_;
    bool preparation_stops_replay_;
    std::string last_session_error_;
    std::uint64_t performance_instruction_baseline_;
    QLabel* machine_status_;
    QLabel* execution_error_status_;
    QLabel* notification_status_;
    QLabel* performance_status_;
    QLabel* session_status_;
    QLabel* runtime_status_;
};

UiExitState run_gui(const UiSession& session, const UiStartupState& startup);

} // namespace se_ui
