#include "se_ui/main_window.h"

#include "se_ui/debugger/cache_dock.h"
#include "se_ui/debugger/disassembly_dock.h"
#include "se_ui/debugger/memory_dock.h"
#include "se_ui/debugger/registers_dock.h"
#include "se_ui/debugger/tlb_dock.h"
#include "se_ui/display_widget.h"
#include "se_ui/src/bridge.rs.h"

#include <QAction>
#include <QApplication>
#include <QByteArray>
#include <QCoreApplication>
#include <QIcon>
#include <QKeySequence>
#include <QMenu>
#include <QMenuBar>
#include <QStyle>
#include <QToolBar>

#include <cstddef>

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

} // namespace

MainWindow::MainWindow(const UiStartupState& startup)
    : settings_ {
          from_rust_string(startup.machine_model),
          from_rust_string(startup.prom_path),
          from_rust_string(startup.float_backend),
      }
    , run_action_(nullptr)
    , reset_action_(nullptr)
    , pause_action_(nullptr)
    , step_action_(nullptr)
    , settings_action_(nullptr)
    , disassembly_dock_(nullptr)
    , registers_dock_(nullptr)
    , tlb_dock_(nullptr)
    , cache_dock_(nullptr)
    , memory_dock_(nullptr) {
    setObjectName(QStringLiteral("MainWindow"));
    setWindowTitle(QStringLiteral("sgi-emu"));
    setCentralWidget(new DisplayWidget(this));
    resize(1100, 720);

    create_actions();
    create_docks();
    create_menus();
    create_toolbar();
    set_default_dock_layout();
    restore_window_state(startup);
}

UiExitState MainWindow::exit_state() const {
    return {
        to_rust_string(settings_.machine_model),
        to_rust_string(settings_.prom_path),
        to_rust_string(settings_.float_backend),
        encoded_bytes(saveGeometry()),
        encoded_bytes(saveState()),
    };
}

void MainWindow::create_actions() {
    run_action_ = new QAction(
        style()->standardIcon(QStyle::SP_MediaPlay), QStringLiteral("Run"), this);
    run_action_->setShortcut(QKeySequence(QStringLiteral("F5")));

    reset_action_ = new QAction(
        style()->standardIcon(QStyle::SP_BrowserReload), QStringLiteral("Reset"), this);
    reset_action_->setShortcut(QKeySequence(QStringLiteral("Ctrl+Shift+F5")));

    pause_action_ = new QAction(
        style()->standardIcon(QStyle::SP_MediaPause), QStringLiteral("Pause"), this);
    pause_action_->setShortcut(QKeySequence(QStringLiteral("F6")));

    step_action_ = new QAction(
        style()->standardIcon(QStyle::SP_MediaSkipForward), QStringLiteral("Step"), this);
    step_action_->setShortcut(QKeySequence(QStringLiteral("F10")));

    settings_action_ = new QAction(
        style()->standardIcon(QStyle::SP_FileDialogDetailedView),
        QStringLiteral("Settings"),
        this);
    settings_action_->setShortcut(QKeySequence::Preferences);
    connect(settings_action_, &QAction::triggered, this, &MainWindow::show_settings);

    run_action_->setEnabled(false);
    reset_action_->setEnabled(false);
    pause_action_->setEnabled(false);
    step_action_->setEnabled(false);
}

void MainWindow::create_docks() {
    disassembly_dock_ = new DisassemblyDock(this);
    registers_dock_ = new RegistersDock(this);
    tlb_dock_ = new TlbDock(this);
    cache_dock_ = new CacheDock(this);
    memory_dock_ = new MemoryDock(this);
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
    view_menu->addAction(disassembly_dock_->toggleViewAction());
    view_menu->addAction(registers_dock_->toggleViewAction());
    view_menu->addAction(tlb_dock_->toggleViewAction());
    view_menu->addAction(cache_dock_->toggleViewAction());
    view_menu->addAction(memory_dock_->toggleViewAction());
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

    disassembly_dock_->hide();
    registers_dock_->hide();
    tlb_dock_->hide();
    cache_dock_->hide();
    memory_dock_->hide();
}

void MainWindow::show_settings() {
    SettingsDialog dialog(settings_, this);
    if (dialog.exec() == QDialog::Accepted) {
        settings_ = dialog.settings();
    }
}

UiExitState run_gui(const UiStartupState& startup) {
    int argument_count = 1;
    char application_name[] = "sgi-emu";
    char* arguments[] = {application_name, nullptr};
    QApplication application(argument_count, arguments);
    QCoreApplication::setApplicationName(QStringLiteral("sgi-emu"));

    MainWindow window(startup);
    window.show();
    application.exec();
    return window.exit_state();
}

} // namespace se_ui
