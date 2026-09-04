#include "se_ui/main_window.h"

#include "se_ui/debugger/cache_dock.h"
#include "se_ui/debugger/disassembly_dock.h"
#include "se_ui/debugger/memory_dock.h"
#include "se_ui/debugger/registers_dock.h"
#include "se_ui/debugger/tlb_dock.h"
#include "se_ui/display_widget.h"
#include "se_ui/serial_console_dock.h"
#include "se_ui/src/bridge.rs.h"

#include <QAction>
#include <QApplication>
#include <QByteArray>
#include <QCoreApplication>
#include <QFileDialog>
#include <QFileInfo>
#include <QIcon>
#include <QKeySequence>
#include <QLabel>
#include <QMenu>
#include <QMenuBar>
#include <QMessageBox>
#include <QSizePolicy>
#include <QStatusBar>
#include <QStyle>
#include <QTimer>
#include <QToolBar>

#include <cstddef>
#include <chrono>
#include <functional>
#include <future>

namespace se_ui {
namespace {

QString from_rust_string(const rust::String& value) {
    return QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size()));
}

rust::String to_rust_string(const QString& value) {
    const auto utf8 = value.toUtf8();
    return rust::String(utf8.constData(), static_cast<std::size_t>(utf8.size()));
}

rust::String encoded_bytes(const QByteArray& value) {
    const auto encoded = value.toBase64();
    return rust::String(encoded.constData(), static_cast<std::size_t>(encoded.size()));
}

QByteArray decoded_bytes(const rust::String& value) {
    return QByteArray::fromBase64(
        QByteArray(value.data(), static_cast<qsizetype>(value.size())));
}

MachineSettings from_machine_configuration(const MachineConfiguration& configuration) {
    return {
        from_rust_string(configuration.machine_model),
        from_rust_string(configuration.prom_path),
        from_rust_string(configuration.disk_path),
        from_rust_string(configuration.cdrom_path),
        from_rust_string(configuration.float_backend),
    };
}

MachineConfiguration to_machine_configuration(const MachineSettings& settings) {
    return {
        to_rust_string(settings.machine_model),
        to_rust_string(settings.prom_path),
        to_rust_string(settings.disk_path),
        to_rust_string(settings.cdrom_path),
        to_rust_string(settings.float_backend),
    };
}

} // namespace

class PreparationTask final {
public:
    explicit PreparationTask(std::function<RuntimeStatusDto()> command)
        : future(std::async(std::launch::async, std::move(command))) {
    }

    std::future<RuntimeStatusDto> future;
};

MainWindow::MainWindow(const UiSession& session, const UiStartupState& startup)
    : session_(session)
    , settings_(from_machine_configuration(startup.machine))
    , run_action_(nullptr)
    , run_with_record_action_(nullptr)
    , reset_action_(nullptr)
    , pause_action_(nullptr)
    , step_action_(nullptr)
    , stop_recording_action_(nullptr)
    , open_replay_action_(nullptr)
    , stop_replay_action_(nullptr)
    , settings_action_(nullptr)
    , disassembly_dock_(nullptr)
    , registers_dock_(nullptr)
    , tlb_dock_(nullptr)
    , cache_dock_(nullptr)
    , memory_dock_(nullptr)
    , serial_console_dock_(nullptr)
    , machine_output_sink_()
    , update_timer_(new QTimer(this))
    , notification_timer_(new QTimer(this))
    , performance_timer_()
    , preparation_task_()
    , preparation_state_(PreparationState::None)
    , preparation_resume_running_(false)
    , preparation_stops_replay_(false)
    , last_session_error_()
    , performance_instruction_baseline_(0)
    , machine_status_(new QLabel(this))
    , execution_error_status_(new QLabel(this))
    , notification_status_(new QLabel(this))
    , performance_status_(new QLabel(this))
    , session_status_(new QLabel(this))
    , runtime_status_(new QLabel(this)) {
    setObjectName(QStringLiteral("MainWindow"));
    setWindowTitle(QStringLiteral("sgi-emu"));
    setCentralWidget(new DisplayWidget(this));
    resize(1100, 720);

    create_actions();
    create_docks();
    create_menus();
    create_toolbar();
    create_status_bar();
    set_default_dock_layout();
    restore_window_state(startup);

    machine_output_sink_ = std::make_shared<MachineOutputSink>(serial_console_dock_);
    apply_runtime_status(session_.attach_machine_output(machine_output_sink_), true);

    connect(update_timer_, &QTimer::timeout, this, &MainWindow::update_runtime);
    update_timer_->start(100);
    apply_runtime_status(session_.runtime_status(), false);

    const auto startup_error = from_rust_string(startup.startup_error);
    if (!startup_error.isEmpty()) {
        QTimer::singleShot(0, this, [this, startup_error] {
            QMessageBox::critical(this, QStringLiteral("Machine configuration"), startup_error);
        });
    }
}

MainWindow::~MainWindow() {
    session_.detach_machine_output();
    machine_output_sink_.reset();
}

UiExitState MainWindow::exit_state() const {
    return {
        to_machine_configuration(settings_),
        encoded_bytes(saveGeometry()),
        encoded_bytes(saveState()),
    };
}

void MainWindow::create_actions() {
    run_action_ = new QAction(
        style()->standardIcon(QStyle::SP_MediaPlay), QStringLiteral("Run"), this);
    run_action_->setShortcut(QKeySequence(QStringLiteral("F5")));
    connect(run_action_, &QAction::triggered, this, [this] {
        apply_runtime_status(session_.run_machine(), true);
    });

    run_with_record_action_ = new QAction(QStringLiteral("Run with Record"), this);
    connect(
        run_with_record_action_, &QAction::triggered, this, &MainWindow::run_with_record);

    reset_action_ = new QAction(
        style()->standardIcon(QStyle::SP_BrowserReload), QStringLiteral("Reset"), this);
    reset_action_->setShortcut(QKeySequence(QStringLiteral("Ctrl+Shift+F5")));
    connect(reset_action_, &QAction::triggered, this, [this] {
        apply_runtime_status(session_.reset_machine(), true);
        refresh_debuggers();
        cache_dock_->clear();
    });

    pause_action_ = new QAction(
        style()->standardIcon(QStyle::SP_MediaPause), QStringLiteral("Pause"), this);
    pause_action_->setShortcut(QKeySequence(QStringLiteral("F6")));
    connect(pause_action_, &QAction::triggered, this, [this] {
        apply_runtime_status(session_.pause_machine(), true);
        refresh_debuggers();
    });

    step_action_ = new QAction(
        style()->standardIcon(QStyle::SP_MediaSkipForward), QStringLiteral("Step"), this);
    step_action_->setShortcut(QKeySequence(QStringLiteral("F10")));
    connect(step_action_, &QAction::triggered, this, [this] {
        apply_runtime_status(session_.step_machine(), true);
        refresh_debuggers();
    });

    stop_recording_action_ = new QAction(QStringLiteral("Stop Recording"), this);
    connect(
        stop_recording_action_, &QAction::triggered, this, &MainWindow::stop_recording);

    open_replay_action_ = new QAction(QStringLiteral("Open Replay"), this);
    connect(open_replay_action_, &QAction::triggered, this, &MainWindow::open_replay);

    stop_replay_action_ = new QAction(QStringLiteral("Stop Replay"), this);
    connect(stop_replay_action_, &QAction::triggered, this, &MainWindow::stop_replay);

    settings_action_ = new QAction(
        style()->standardIcon(QStyle::SP_FileDialogDetailedView),
        QStringLiteral("Settings"),
        this);
    settings_action_->setShortcut(QKeySequence::Preferences);
    connect(settings_action_, &QAction::triggered, this, &MainWindow::show_settings);
}

void MainWindow::create_docks() {
    disassembly_dock_ = new DisassemblyDock(session_, this);
    registers_dock_ = new RegistersDock(
        session_,
        [this](const QString& message) { show_notification(message, 3000); },
        this);
    tlb_dock_ = new TlbDock(session_, this);
    cache_dock_ = new CacheDock(session_, this);
    memory_dock_ = new MemoryDock(session_, this);
    serial_console_dock_ = new SerialConsoleDock(
        session_,
        [this](const RuntimeStatusDto& status) { apply_runtime_status(status, false); },
        this);
}

void MainWindow::create_menus() {
    auto* machine_menu = menuBar()->addMenu(QStringLiteral("Machine"));
    machine_menu->addAction(run_action_);
    machine_menu->addAction(reset_action_);
    machine_menu->addAction(pause_action_);
    machine_menu->addAction(step_action_);
    machine_menu->addSeparator();
    machine_menu->addAction(settings_action_);

    auto* view_menu = menuBar()->addMenu(QStringLiteral("View"));
    view_menu->addAction(serial_console_dock_->toggleViewAction());
    view_menu->addSeparator();
    view_menu->addAction(disassembly_dock_->toggleViewAction());
    view_menu->addAction(registers_dock_->toggleViewAction());
    view_menu->addAction(tlb_dock_->toggleViewAction());
    view_menu->addAction(cache_dock_->toggleViewAction());
    view_menu->addAction(memory_dock_->toggleViewAction());

    auto* debug_menu = menuBar()->addMenu(QStringLiteral("Debug"));
    debug_menu->addAction(run_with_record_action_);
    debug_menu->addAction(stop_recording_action_);
    debug_menu->addSeparator();
    debug_menu->addAction(open_replay_action_);
    debug_menu->addAction(stop_replay_action_);
}

void MainWindow::create_toolbar() {
    auto* toolbar = addToolBar(QStringLiteral("Machine"));
    toolbar->setObjectName(QStringLiteral("MachineToolbar"));
    toolbar->setMovable(false);
    toolbar->addAction(run_action_);
    toolbar->addAction(reset_action_);
    toolbar->addAction(pause_action_);
    toolbar->addAction(step_action_);
    toolbar->addSeparator();
    toolbar->addAction(settings_action_);
}

void MainWindow::create_status_bar() {
    notification_timer_->setSingleShot(true);
    connect(notification_timer_, &QTimer::timeout, this, [this] {
        notification_status_->clear();
        notification_status_->hide();
    });
    execution_error_status_->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    statusBar()->addWidget(machine_status_);
    statusBar()->addWidget(execution_error_status_, 1);
    statusBar()->addPermanentWidget(notification_status_);
    statusBar()->addPermanentWidget(performance_status_);
    statusBar()->addPermanentWidget(session_status_);
    statusBar()->addPermanentWidget(runtime_status_);
    execution_error_status_->hide();
    notification_status_->hide();
    performance_status_->setText(QStringLiteral("IPS: \u2014"));
    session_status_->setText(QStringLiteral("Session: Normal"));
    runtime_status_->setText(QStringLiteral("State: Unconfigured"));
    update_machine_status();
}

void MainWindow::show_notification(const QString& message, int timeout) {
    notification_timer_->stop();
    notification_status_->setText(message);
    notification_status_->setToolTip(message);
    notification_status_->setVisible(!message.isEmpty());
    if (!message.isEmpty() && timeout > 0) {
        notification_timer_->start(timeout);
    }
}

void MainWindow::begin_preparation(
    PreparationState state,
    bool stops_replay,
    std::function<RuntimeStatusDto()> command) {
    if (preparation_state_ != PreparationState::None) {
        return;
    }
    const auto current = session_.runtime_status();
    preparation_resume_running_ = current.success && current.state == 2;
    if (preparation_resume_running_) {
        session_.pause_machine();
    }
    preparation_state_ = state;
    preparation_stops_replay_ = stops_replay;
    preparation_task_ = std::make_unique<PreparationTask>(std::move(command));
    apply_preparation_state();
}

void MainWindow::poll_preparation() {
    if (preparation_state_ == PreparationState::None || preparation_task_ == nullptr
        || preparation_task_->future.wait_for(std::chrono::seconds(0))
            != std::future_status::ready) {
        return;
    }

    const auto completed_state = preparation_state_;
    const bool stopped_replay = preparation_stops_replay_;
    const bool resume_running = preparation_resume_running_;
    const auto status = preparation_task_->future.get();
    preparation_task_.reset();
    preparation_state_ = PreparationState::None;
    preparation_resume_running_ = false;
    preparation_stops_replay_ = false;
    apply_runtime_status(status, false);
    if (!status.success) {
        if (resume_running) {
            apply_runtime_status(session_.run_machine(), false);
        }
        return;
    }

    if (completed_state == PreparationState::Recording) {
        show_notification(QStringLiteral("Recording started"), 3000);
    } else if (stopped_replay) {
        show_notification(QStringLiteral("Replay stopped"), 3000);
    }
    registers_dock_->clear();
    tlb_dock_->clear();
    cache_dock_->clear();
    disassembly_dock_->clear();
    memory_dock_->clear();
    refresh_debuggers();
}

void MainWindow::apply_preparation_state() {
    if (preparation_state_ == PreparationState::None) {
        return;
    }
    run_action_->setEnabled(false);
    run_with_record_action_->setEnabled(false);
    reset_action_->setEnabled(false);
    pause_action_->setEnabled(false);
    step_action_->setEnabled(false);
    stop_recording_action_->setEnabled(false);
    open_replay_action_->setEnabled(false);
    stop_replay_action_->setEnabled(false);
    settings_action_->setEnabled(false);
    serial_console_dock_->set_input_enabled(false);
    session_status_->setText(
        preparation_state_ == PreparationState::Recording
            ? QStringLiteral("Preparing recording...")
            : QStringLiteral("Preparing replay..."));
}

void MainWindow::restore_window_state(const UiStartupState& startup) {
    if (!startup.window_geometry.empty()) {
        restoreGeometry(decoded_bytes(startup.window_geometry));
    }
    if (!startup.window_state.empty() && !restoreState(decoded_bytes(startup.window_state))) {
        set_default_dock_layout();
    }
}

void MainWindow::set_default_dock_layout() {
    addDockWidget(Qt::LeftDockWidgetArea, disassembly_dock_);
    addDockWidget(Qt::RightDockWidgetArea, registers_dock_);
    addDockWidget(Qt::RightDockWidgetArea, tlb_dock_);
    addDockWidget(Qt::RightDockWidgetArea, cache_dock_);
    tabifyDockWidget(registers_dock_, tlb_dock_);
    tabifyDockWidget(registers_dock_, cache_dock_);
    addDockWidget(Qt::BottomDockWidgetArea, memory_dock_);
    addDockWidget(Qt::BottomDockWidgetArea, serial_console_dock_);
    resizeDocks({serial_console_dock_}, {480}, Qt::Vertical);

    disassembly_dock_->hide();
    registers_dock_->hide();
    tlb_dock_->hide();
    cache_dock_->hide();
    memory_dock_->hide();
    serial_console_dock_->show();
}

void MainWindow::run_with_record() {
    auto path = QFileDialog::getSaveFileName(
        this,
        QStringLiteral("Run with record"),
        QString(),
        QStringLiteral("Record file (*.serec)"));
    if (path.isEmpty()) {
        return;
    }
    if (QFileInfo(path).suffix().compare(QStringLiteral("serec"), Qt::CaseInsensitive) != 0) {
        path += QStringLiteral(".serec");
    }
    if (QMessageBox::question(
            this,
            QStringLiteral("Cold-start recording"),
            QStringLiteral(
                "Recording cold-starts the machine and discards its volatile state. "
                "Guest disk writes will still modify the selected disk image. Continue?"))
        != QMessageBox::Yes) {
        return;
    }

    auto configuration =
        std::make_shared<MachineConfiguration>(to_machine_configuration(settings_));
    auto record_path = std::make_shared<rust::String>(to_rust_string(path));
    begin_preparation(
        PreparationState::Recording,
        false,
        [this, configuration, record_path] {
            return session_.run_with_record(*configuration, rust::Str(*record_path));
        });
}

void MainWindow::stop_recording() {
    const auto status = session_.stop_recording();
    apply_runtime_status(status, true);
    if (status.success) {
        show_notification(QStringLiteral("Recording saved"), 3000);
    }
}

void MainWindow::open_replay() {
    const auto path = QFileDialog::getOpenFileName(
        this,
        QStringLiteral("Open Replay"),
        QString(),
        QStringLiteral("sgi-emu Record (*.serec)"));
    if (path.isEmpty()) {
        return;
    }
    auto configuration =
        std::make_shared<MachineConfiguration>(to_machine_configuration(settings_));
    auto replay_path = std::make_shared<rust::String>(to_rust_string(path));
    begin_preparation(
        PreparationState::Replay,
        false,
        [this, configuration, replay_path] {
            return session_.open_replay(*configuration, rust::Str(*replay_path));
        });
}

void MainWindow::stop_replay() {
    auto configuration =
        std::make_shared<MachineConfiguration>(to_machine_configuration(settings_));
    begin_preparation(PreparationState::Replay, true, [this, configuration] {
        return session_.stop_replay(*configuration);
    });
}

void MainWindow::show_settings() {
    SettingsDialog dialog(settings_, this);
    if (dialog.exec() != QDialog::Accepted) {
        return;
    }

    const auto selected = dialog.settings();
    if (selected.machine_model == settings_.machine_model
        && selected.prom_path == settings_.prom_path
        && selected.disk_path == settings_.disk_path
        && selected.cdrom_path == settings_.cdrom_path
        && selected.float_backend == settings_.float_backend) {
        return;
    }
    if (QMessageBox::question(
            this,
            QStringLiteral("Reset machine"),
            QStringLiteral("Changing these settings will reset the emulated machine. Continue?"))
        != QMessageBox::Yes) {
        return;
    }

    const auto configuration = to_machine_configuration(selected);
    const auto status = session_.configure_machine(configuration);
    if (!status.success) {
        QMessageBox::critical(
            this,
            QStringLiteral("Machine configuration"),
            from_rust_string(status.command_error));
        return;
    }

    settings_ = selected;
    update_machine_status();
    registers_dock_->clear();
    tlb_dock_->clear();
    cache_dock_->clear();
    disassembly_dock_->clear();
    memory_dock_->clear();
    apply_runtime_status(status, false);
    refresh_debuggers();
}

void MainWindow::update_runtime() {
    poll_preparation();
    const auto status = session_.runtime_status();
    apply_runtime_status(status, false);
    apply_preparation_state();
    refresh_debuggers();
}

void MainWindow::refresh_debuggers() {
    if (!isVisible() || isMinimized()) {
        return;
    }
    if (registers_dock_->isVisible()) {
        registers_dock_->refresh();
    }
    if (tlb_dock_->isVisible()) {
        tlb_dock_->refresh();
    }
    if (disassembly_dock_->isVisible()) {
        disassembly_dock_->refresh();
    }
    if (memory_dock_->isVisible()) {
        memory_dock_->refresh();
    }
}

void MainWindow::apply_runtime_status(const RuntimeStatusDto& status, bool report_error) {
    if (!status.success) {
        const auto command_error = from_rust_string(status.command_error);
        show_notification(QStringLiteral("Error: %1").arg(command_error), 5000);
        if (report_error) {
            QMessageBox::warning(
                this, QStringLiteral("Machine command"), command_error);
        }
        return;
    }

    update_performance_status(status);

    const bool configured = status.state != 0;
    const bool paused = status.state == 1;
    const bool running = status.state == 2;
    const bool normal = status.mode == 0;
    const bool recording = status.mode == 1;
    const bool replaying = status.mode == 2;
    const bool replay_session = replaying || status.mode == 3 || status.mode == 4;
    const bool session_stopped = status.mode == 3 || status.mode == 4;
    run_action_->setEnabled(paused && !session_stopped);
    run_with_record_action_->setEnabled(configured && normal);
    reset_action_->setEnabled(configured && !replay_session);
    pause_action_->setEnabled(running);
    step_action_->setEnabled(paused && !session_stopped);
    stop_recording_action_->setEnabled(recording);
    open_replay_action_->setEnabled(normal);
    stop_replay_action_->setEnabled(replay_session);
    settings_action_->setEnabled(normal);
    serial_console_dock_->set_input_enabled(!replay_session);

    const auto session_error = from_rust_string(status.session_error);
    const auto session_error_utf8 = session_error.toUtf8();
    const std::string session_error_text(
        session_error_utf8.constData(), static_cast<std::size_t>(session_error_utf8.size()));
    if (session_error_text != last_session_error_) {
        last_session_error_ = session_error_text;
        if (!session_error.isEmpty()) {
            show_notification(QStringLiteral("Error: %1").arg(session_error), 5000);
        }
    }
    if (recording) {
        session_status_->setText(QStringLiteral("Session: Recording"));
    } else if (replaying) {
        session_status_->setText(
            status.has_replay_final_position
                ? QStringLiteral("Session: Replay %1:%2 / %3:%4")
                      .arg(status.epoch)
                      .arg(status.epoch_instructions)
                      .arg(status.replay_final_epoch)
                      .arg(status.replay_final_instructions)
                : QStringLiteral("Session: Replay %1:%2")
                      .arg(status.epoch)
                      .arg(status.epoch_instructions));
    } else if (status.mode == 3) {
        session_status_->setText(QStringLiteral("Session: Replay complete"));
    } else if (status.mode == 4) {
        session_status_->setText(QStringLiteral("Session: Replay diverged"));
    } else if (!session_error.isEmpty()) {
        session_status_->setText(QStringLiteral("Session: Record failed"));
    } else {
        session_status_->setText(QStringLiteral("Session: Normal"));
    }
    session_status_->setToolTip(session_error);

    const auto execution_error = from_rust_string(status.execution_error);
    execution_error_status_->setText(
        execution_error.isEmpty() ? QString() : QStringLiteral("Error: %1").arg(execution_error));
    execution_error_status_->setToolTip(execution_error);
    execution_error_status_->setVisible(!execution_error.isEmpty());

    if (running) {
        runtime_status_->setText(QStringLiteral("State: Running"));
    } else if (paused) {
        runtime_status_->setText(QStringLiteral("State: Paused"));
    } else {
        runtime_status_->setText(QStringLiteral("State: Unconfigured"));
    }
}

void MainWindow::update_performance_status(const RuntimeStatusDto& status) {
    if (status.state != 2) {
        performance_timer_.invalidate();
        performance_status_->setText(QStringLiteral("IPS: \u2014"));
        return;
    }

    if (!performance_timer_.isValid()) {
        performance_instruction_baseline_ = status.completed_instructions;
        performance_timer_.start();
        return;
    }

    const auto elapsed_milliseconds = performance_timer_.elapsed();
    if (elapsed_milliseconds < 1000) {
        return;
    }

    const auto completed = status.completed_instructions - performance_instruction_baseline_;
    const auto instructions_per_second = static_cast<double>(completed) * 1000.0
        / static_cast<double>(elapsed_milliseconds);
    QString formatted_rate;
    if (instructions_per_second >= 1'000'000.0) {
        formatted_rate = QStringLiteral("%1M").arg(instructions_per_second / 1'000'000.0, 0, 'f', 2);
    } else if (instructions_per_second >= 1'000.0) {
        formatted_rate = QStringLiteral("%1K").arg(instructions_per_second / 1'000.0, 0, 'f', 1);
    } else {
        formatted_rate = QString::number(instructions_per_second, 'f', 0);
    }
    performance_status_->setText(QStringLiteral("IPS: %1").arg(formatted_rate));
    performance_instruction_baseline_ = status.completed_instructions;
    performance_timer_.restart();
}

void MainWindow::update_machine_status() {
    const auto machine = settings_.machine_model == QStringLiteral("indigo-ip12")
        ? QStringLiteral("Indigo IP12")
        : settings_.machine_model;
    machine_status_->setText(machine);
}

UiExitState run_gui(const UiSession& session, const UiStartupState& startup) {
    int argument_count = 1;
    char application_name[] = "sgi-emu";
    char* arguments[] = {application_name, nullptr};
    QApplication application(argument_count, arguments);
    QCoreApplication::setApplicationName(QStringLiteral("sgi-emu"));

    MainWindow window(session, startup);
    window.show();
    application.exec();
    return window.exit_state();
}

} // namespace se_ui
