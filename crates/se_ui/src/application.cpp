#include "se_ui/include/application.h"
#include "se_ui/include/terminal_dock.h"
#include "se_ui/include/tracing_dock.h"
#include "se_ui/src/application.rs.h"

#include <cstdint>
#include <vector>

#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QEvent>
#include <QtCore/QFile>
#include <QtCore/QFileInfo>
#include <QtCore/QLocale>
#include <QtCore/QSettings>
#include <QtCore/QSignalBlocker>
#include <QtCore/QSize>
#include <QtCore/QString>
#include <QtCore/QTimer>
#include <QtCore/QTranslator>
#include <QtCore/QtGlobal>
#include <QtCore/QtResource>
#include <QtGui/QAction>
#include <QtGui/QCloseEvent>
#include <QtGui/QIcon>
#include <QtGui/QPainter>
#include <QtGui/QPalette>
#include <QtGui/QPixmap>
#include <QtSvg/QSvgRenderer>
#include <QtWidgets/QApplication>
#include <QtWidgets/QComboBox>
#include <QtWidgets/QDialog>
#include <QtWidgets/QDialogButtonBox>
#include <QtWidgets/QDockWidget>
#include <QtWidgets/QFileDialog>
#include <QtWidgets/QFormLayout>
#include <QtWidgets/QHBoxLayout>
#include <QtWidgets/QLabel>
#include <QtWidgets/QLineEdit>
#include <QtWidgets/QMainWindow>
#include <QtWidgets/QMenu>
#include <QtWidgets/QMenuBar>
#include <QtWidgets/QMessageBox>
#include <QtWidgets/QPushButton>
#include <QtWidgets/QStatusBar>
#include <QtWidgets/QStyle>
#include <QtWidgets/QStyleFactory>
#include <QtWidgets/QToolBar>
#include <QtWidgets/QWidget>

void initialize_resources()
{
  Q_INIT_RESOURCE(se_ui_translations_qrc);
}

namespace se::ui {
namespace {

constexpr auto action_text = QT_TRANSLATE_NOOP("MainWindow", "Action");
constexpr auto view_text = QT_TRANSLATE_NOOP("MainWindow", "View");
constexpr auto window_text = QT_TRANSLATE_NOOP("MainWindow", "Window");
constexpr auto tools_text = QT_TRANSLATE_NOOP("MainWindow", "Tools");
constexpr auto help_text = QT_TRANSLATE_NOOP("MainWindow", "Help");
constexpr auto run_text = QT_TRANSLATE_NOOP("MainWindow", "Run");
constexpr auto pause_text = QT_TRANSLATE_NOOP("MainWindow", "Pause");
constexpr auto hard_reset_text = QT_TRANSLATE_NOOP("MainWindow", "Hard Reset");
constexpr auto save_state_text = QT_TRANSLATE_NOOP("MainWindow", "Save State...");
constexpr auto load_state_text = QT_TRANSLATE_NOOP("MainWindow", "Load State...");
constexpr auto save_state_title_text = QT_TRANSLATE_NOOP("MainWindow", "Save Emulator State");
constexpr auto load_state_title_text = QT_TRANSLATE_NOOP("MainWindow", "Load Emulator State");
constexpr auto state_filter_text = QT_TRANSLATE_NOOP("MainWindow", "sgi-emu states (*.sestate)");
constexpr auto replace_machine_text = QT_TRANSLATE_NOOP(
  "MainWindow",
  "Loading this state will replace the current machine session. Continue?");
constexpr auto select_matching_prom_text = QT_TRANSLATE_NOOP(
  "MainWindow",
  "Select the 512 KiB System PROM matching this state file.");
constexpr auto persistence_failed_text = QT_TRANSLATE_NOOP("MainWindow", "Persistence Error");
constexpr auto persistence_warning_text = QT_TRANSLATE_NOOP("MainWindow", "Persistence Warning");
constexpr auto state_saved_text = QT_TRANSLATE_NOOP("MainWindow", "State saved.");
constexpr auto state_loaded_text = QT_TRANSLATE_NOOP("MainWindow", "State loaded.");
constexpr auto hide_toolbar_text = QT_TRANSLATE_NOOP("MainWindow", "Hide Toolbar");
constexpr auto toolbar_text = QT_TRANSLATE_NOOP("MainWindow", "Toolbar");
constexpr auto hide_status_bar_text = QT_TRANSLATE_NOOP("MainWindow", "Hide Status Bar");
constexpr auto animated_docks_text = QT_TRANSLATE_NOOP("MainWindow", "Animated Docks");
constexpr auto allow_nested_docks_text =
  QT_TRANSLATE_NOOP("MainWindow", "Allow Nested Docks");
constexpr auto allow_tabbed_docks_text =
  QT_TRANSLATE_NOOP("MainWindow", "Allow Tabbed Docks");
constexpr auto settings_text = QT_TRANSLATE_NOOP("MainWindow", "Settings");
constexpr auto qt_ui_style_text = QT_TRANSLATE_NOOP("MainWindow", "Qt UI Style");
constexpr auto emulation_settings_text =
  QT_TRANSLATE_NOOP("MainWindow", "Emulation Settings");
constexpr auto about_text = QT_TRANSLATE_NOOP("MainWindow", "About");
constexpr auto machine_text = QT_TRANSLATE_NOOP("MainWindow", "Machine");
constexpr auto ip32_machine_text =
  QT_TRANSLATE_NOOP("MainWindow", "SGI O2 (IP32)");
constexpr auto system_prom_text =
  QT_TRANSLATE_NOOP("MainWindow", "System PROM");
constexpr auto rtc_mode_text = QT_TRANSLATE_NOOP("MainWindow", "RTC Persistence");
constexpr auto rtc_real_time_text = QT_TRANSLATE_NOOP("MainWindow", "Real Time");
constexpr auto rtc_frozen_text = QT_TRANSLATE_NOOP("MainWindow", "Frozen");
constexpr auto rtc_sync_host_text =
  QT_TRANSLATE_NOOP("MainWindow", "Synchronize with Host");
constexpr auto browse_text = QT_TRANSLATE_NOOP("MainWindow", "Browse...");
constexpr auto select_prom_text =
  QT_TRANSLATE_NOOP("MainWindow", "Select System PROM");
constexpr auto prom_filter_text =
  QT_TRANSLATE_NOOP("MainWindow", "PROM images (*)");
constexpr auto prom_required_text =
  QT_TRANSLATE_NOOP("MainWindow", "Select a System PROM image.");
constexpr auto prom_read_failed_text =
  QT_TRANSLATE_NOOP("MainWindow", "Failed to read the selected System PROM.");
constexpr auto prom_size_text = QT_TRANSLATE_NOOP(
  "MainWindow",
  "The System PROM must be exactly 524,288 bytes.");
constexpr auto configure_failed_text = QT_TRANSLATE_NOOP(
  "MainWindow",
  "The emulator cannot apply this configuration in its current state.");
constexpr auto emulation_error_text =
  QT_TRANSLATE_NOOP("MainWindow", "Emulation Error");
constexpr auto ip32_status_text = QT_TRANSLATE_NOOP("MainWindow", "IP32: %1");
constexpr auto unconfigured_text =
  QT_TRANSLATE_NOOP("MainWindow", "Unconfigured");
constexpr auto building_text = QT_TRANSLATE_NOOP("MainWindow", "Building");
constexpr auto saving_text = QT_TRANSLATE_NOOP("MainWindow", "Saving");
constexpr auto loading_text = QT_TRANSLATE_NOOP("MainWindow", "Loading");
constexpr auto paused_text = QT_TRANSLATE_NOOP("MainWindow", "Paused");
constexpr auto running_text = QT_TRANSLATE_NOOP("MainWindow", "Running");
constexpr auto idle_text = QT_TRANSLATE_NOOP("MainWindow", "Idle");
constexpr auto faulted_text = QT_TRANSLATE_NOOP("MainWindow", "Faulted");
constexpr auto shutting_down_text =
  QT_TRANSLATE_NOOP("MainWindow", "Shutting Down");

constexpr auto run_icon_path = ":/icons/run.svg";
constexpr auto pause_icon_path = ":/icons/pause.svg";
constexpr auto hard_reset_icon_path = ":/icons/hard-reset.svg";
constexpr auto emulation_settings_icon_path =
  ":/icons/emulation-settings.svg";
constexpr auto ui_settings_schema = 1;

QString translate(const char* source)
{
  return QCoreApplication::translate("MainWindow", source);
}

QString from_rust_string(const rust::String& value)
{
  return QString::fromUtf8(
    value.data(),
    static_cast<qsizetype>(value.size()));
}

QPixmap tinted_svg_pixmap(
  const char* path,
  int size,
  const QColor& color)
{
  QSvgRenderer renderer(QString::fromLatin1(path));
  QPixmap pixmap(size, size);
  pixmap.fill(Qt::transparent);

  QPainter painter(&pixmap);
  renderer.render(&painter);
  painter.setCompositionMode(QPainter::CompositionMode_SourceIn);
  painter.fillRect(pixmap.rect(), color);
  return pixmap;
}

QIcon palette_icon(const char* path, const QPalette& palette)
{
  QIcon icon;
  for (const auto size : { 16, 24, 32, 48 }) {
    icon.addPixmap(
      tinted_svg_pixmap(
        path,
        size,
        palette.color(QPalette::Active, QPalette::WindowText)),
      QIcon::Normal);
    icon.addPixmap(
      tinted_svg_pixmap(
        path,
        size,
        palette.color(QPalette::Disabled, QPalette::WindowText)),
      QIcon::Disabled);
    icon.addPixmap(
      tinted_svg_pixmap(
        path,
        size,
        palette.color(QPalette::Active, QPalette::HighlightedText)),
      QIcon::Selected);
  }
  return icon;
}

QString emulation_state_text(EmulationState state)
{
  switch (state) {
  case EmulationState::Unconfigured:
    return translate(unconfigured_text);
  case EmulationState::Building:
    return translate(building_text);
  case EmulationState::Saving:
    return translate(saving_text);
  case EmulationState::Loading:
    return translate(loading_text);
  case EmulationState::Paused:
    return translate(paused_text);
  case EmulationState::Running:
    return translate(running_text);
  case EmulationState::Idle:
    return translate(idle_text);
  case EmulationState::Faulted:
    return translate(faulted_text);
  case EmulationState::ShuttingDown:
    return translate(shutting_down_text);
  }
  return {};
}

void show_settings_dialog(QWidget* parent)
{
  QDialog dialog(parent);
  dialog.setWindowTitle(translate(settings_text));

  auto* layout = new QFormLayout(&dialog);
  auto* style_selector = new QComboBox(&dialog);
  style_selector->addItems(QStyleFactory::keys());

  const auto current_style = QApplication::style()->objectName();
  for (auto index = 0; index < style_selector->count(); ++index) {
    if (style_selector->itemText(index).compare(
          current_style,
          Qt::CaseInsensitive)
        == 0) {
      style_selector->setCurrentIndex(index);
      break;
    }
  }

  layout->addRow(translate(qt_ui_style_text), style_selector);

  auto* buttons = new QDialogButtonBox(
    QDialogButtonBox::Ok | QDialogButtonBox::Cancel,
    &dialog);
  QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
  QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
  layout->addRow(buttons);

  if (dialog.exec() == QDialog::Accepted) {
    QApplication::setStyle(style_selector->currentText());
    QSettings settings;
    settings.setValue(QStringLiteral("ui/style"), style_selector->currentText());
  }
}

void show_emulation_settings_dialog(
  QWidget* parent,
  const EmulationController& controller)
{
  constexpr qsizetype prom_size = 512 * 1024;

  QDialog dialog(parent);
  dialog.setWindowTitle(translate(emulation_settings_text));

  auto* layout = new QFormLayout(&dialog);
  layout->addRow(translate(machine_text), new QLabel(translate(ip32_machine_text), &dialog));

  auto* prom_row = new QWidget(&dialog);
  auto* prom_layout = new QHBoxLayout(prom_row);
  prom_layout->setContentsMargins(0, 0, 0, 0);
  auto* prom_path = new QLineEdit(prom_row);
  const auto snapshot = controller.snapshot();
  prom_path->setText(from_rust_string(snapshot.prom_path));
  auto* browse = new QPushButton(translate(browse_text), prom_row);
  prom_layout->addWidget(prom_path, 1);
  prom_layout->addWidget(browse);
  layout->addRow(translate(system_prom_text), prom_row);

  auto* rtc_mode = new QComboBox(&dialog);
  rtc_mode->addItem(translate(rtc_real_time_text), 0);
  rtc_mode->addItem(translate(rtc_frozen_text), 1);
  rtc_mode->addItem(translate(rtc_sync_host_text), 2);
  const auto saved_rtc_index = rtc_mode->findData(snapshot.rtc_mode);
  if (saved_rtc_index >= 0) {
    rtc_mode->setCurrentIndex(saved_rtc_index);
  }
  layout->addRow(translate(rtc_mode_text), rtc_mode);

  QObject::connect(browse, &QPushButton::clicked, &dialog, [&dialog, prom_path] {
    QSettings settings;
    const auto path = QFileDialog::getOpenFileName(
      &dialog,
      translate(select_prom_text),
      settings.value(QStringLiteral("recent/promDirectory")).toString(),
      translate(prom_filter_text));
    if (!path.isEmpty()) {
      prom_path->setText(path);
    }
  });

  auto* buttons = new QDialogButtonBox(
    QDialogButtonBox::Ok | QDialogButtonBox::Cancel,
    &dialog);
  QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
  QObject::connect(
    buttons,
    &QDialogButtonBox::accepted,
    &dialog,
    [&dialog, prom_path, rtc_mode, &controller] {
      if (prom_path->text().isEmpty()) {
        QMessageBox::warning(
          &dialog,
          translate(emulation_settings_text),
          translate(prom_required_text));
        return;
      }

      QFile file(prom_path->text());
      if (!file.open(QIODevice::ReadOnly)) {
        QMessageBox::warning(
          &dialog,
          translate(emulation_settings_text),
          translate(prom_read_failed_text));
        return;
      }
      const auto prom = file.readAll();
      if (prom.size() != prom_size) {
        QMessageBox::warning(
          &dialog,
          translate(emulation_settings_text),
          translate(prom_size_text));
        return;
      }

      const auto* data = reinterpret_cast<const std::uint8_t*>(prom.constData());
      const auto absolute_path = QFileInfo(prom_path->text()).absoluteFilePath();
      const auto path_utf8 = absolute_path.toUtf8();
      if (!controller.configure_machine(
            rust::Str(path_utf8.constData(), static_cast<std::size_t>(path_utf8.size())),
            rust::Slice<const std::uint8_t>(
              data,
              static_cast<std::size_t>(prom.size())),
            static_cast<std::uint8_t>(rtc_mode->currentData().toUInt()))) {
        QMessageBox::warning(
          &dialog,
          translate(emulation_settings_text),
          translate(configure_failed_text));
        return;
      }
      QSettings settings;
      settings.setValue(
        QStringLiteral("recent/promDirectory"),
        QFileInfo(absolute_path).absolutePath());
      dialog.accept();
    });
  layout->addRow(buttons);
  dialog.exec();
}

class MainWindow final : public QMainWindow
{
public:
  explicit MainWindow(const EmulationController& controller)
    : controller_(controller)
  {
    setObjectName(QStringLiteral("mainWindow"));
    setWindowTitle(QStringLiteral("%1 %2").arg(
      QCoreApplication::applicationName(),
      QCoreApplication::applicationVersion()));
    resize(1280, 1024);
    setCentralWidget(new QWidget(this));

    auto* action_menu = menuBar()->addMenu(translate(action_text));
    auto* view_menu = menuBar()->addMenu(translate(view_text));
    auto* window_menu = menuBar()->addMenu(translate(window_text));
    auto* tools_menu = menuBar()->addMenu(translate(tools_text));
    auto* help_menu = menuBar()->addMenu(translate(help_text));

    run_pause_action_ = action_menu->addAction(translate(run_text));
    connect(
      run_pause_action_,
      &QAction::triggered,
      this,
      [this] {
        const auto snapshot = controller_.snapshot();
        if (snapshot.state == EmulationState::Running) {
          controller_.request_pause();
        } else if (snapshot.state == EmulationState::Paused) {
          controller_.request_run();
        }
        update_emulation_state();
      });

    hard_reset_action_ = action_menu->addAction(translate(hard_reset_text));
    connect(hard_reset_action_, &QAction::triggered, this, [this] {
      controller_.request_hard_reset();
      update_emulation_state();
    });
    action_menu->addSeparator();
    save_state_action_ = action_menu->addAction(translate(save_state_text));
    connect(save_state_action_, &QAction::triggered, this, [this] {
      QSettings settings;
      const auto path = QFileDialog::getSaveFileName(
        this,
        translate(save_state_title_text),
        settings.value(QStringLiteral("recent/stateSaveDirectory")).toString(),
        translate(state_filter_text));
      if (path.isEmpty()) {
        return;
      }
      auto state_path = path;
      if (QFileInfo(state_path).suffix().isEmpty()) {
        state_path += QStringLiteral(".sestate");
      }
      const auto absolute_path = QFileInfo(state_path).absoluteFilePath();
      const auto utf8 = absolute_path.toUtf8();
      if (controller_.request_save_state(
            rust::Str(utf8.constData(), static_cast<std::size_t>(utf8.size())))) {
        settings.setValue(
          QStringLiteral("recent/stateSaveDirectory"),
          QFileInfo(absolute_path).absolutePath());
      }
      update_emulation_state();
    });
    load_state_action_ = action_menu->addAction(translate(load_state_text));
    connect(load_state_action_, &QAction::triggered, this, [this] {
      const auto snapshot = controller_.snapshot();
      if (snapshot.has_machine
          && QMessageBox::question(
               this,
               translate(load_state_title_text),
               translate(replace_machine_text))
            != QMessageBox::Yes) {
        return;
      }
      QSettings settings;
      const auto path = QFileDialog::getOpenFileName(
        this,
        translate(load_state_title_text),
        settings.value(QStringLiteral("recent/stateLoadDirectory")).toString(),
        translate(state_filter_text));
      if (path.isEmpty()) {
        return;
      }
      pending_load_path_ = QFileInfo(path).absoluteFilePath();
      const auto utf8 = pending_load_path_.toUtf8();
      if (controller_.request_load_state(
            rust::Str(utf8.constData(), static_cast<std::size_t>(utf8.size())),
            rust::Str())) {
        settings.setValue(
          QStringLiteral("recent/stateLoadDirectory"),
          QFileInfo(pending_load_path_).absolutePath());
      }
      update_emulation_state();
    });
    auto* settings_action = tools_menu->addAction(translate(settings_text));
    connect(settings_action, &QAction::triggered, this, [this] {
      show_settings_dialog(this);
    });
    tools_menu->addSeparator();
    emulation_settings_action_ =
      tools_menu->addAction(translate(emulation_settings_text));
    connect(emulation_settings_action_, &QAction::triggered, this, [this] {
      show_emulation_settings_dialog(this, controller_);
      update_emulation_state();
    });

    auto* tool_bar = new QToolBar(this);
    tool_bar->setObjectName(QStringLiteral("mainToolBar"));
    tool_bar->setWindowTitle(translate(toolbar_text));
    tool_bar->setIconSize(QSize(24, 24));
    tool_bar->setToolButtonStyle(Qt::ToolButtonIconOnly);
    addToolBar(Qt::TopToolBarArea, tool_bar);
    tool_bar->addAction(run_pause_action_);
    tool_bar->addAction(hard_reset_action_);
    tool_bar->addSeparator();
    tool_bar->addAction(emulation_settings_action_);

    auto* status_bar = statusBar();
    emulation_status_ = new QLabel(status_bar);
    status_bar->addWidget(emulation_status_);

    auto* hide_tool_bar_action = view_menu->addAction(translate(hide_toolbar_text));
    hide_tool_bar_action->setCheckable(true);
    connect(
      hide_tool_bar_action,
      &QAction::toggled,
      this,
      [tool_bar](bool hidden) { tool_bar->setHidden(hidden); });
    connect(
      tool_bar,
      &QToolBar::visibilityChanged,
      this,
      [hide_tool_bar_action](bool visible) {
        const QSignalBlocker blocker(hide_tool_bar_action);
        hide_tool_bar_action->setChecked(!visible);
      });

    auto* hide_status_bar_action =
      view_menu->addAction(translate(hide_status_bar_text));
    hide_status_bar_action->setCheckable(true);
    connect(
      hide_status_bar_action,
      &QAction::toggled,
      this,
      [status_bar](bool hidden) { status_bar->setHidden(hidden); });

    const auto add_dock_option = [this, window_menu](
                                   const QString& label,
                                   QMainWindow::DockOption option) {
      auto* action = window_menu->addAction(label);
      action->setCheckable(true);
      action->setChecked(dockOptions().testFlag(option));
      connect(action, &QAction::toggled, this, [this, option](bool enabled) {
        auto options = dockOptions();
        options.setFlag(option, enabled);
        setDockOptions(options);
      });
    };

    add_dock_option(translate(animated_docks_text), QMainWindow::AnimatedDocks);
    add_dock_option(translate(allow_nested_docks_text), QMainWindow::AllowNestedDocks);
    add_dock_option(translate(allow_tabbed_docks_text), QMainWindow::AllowTabbedDocks);

    auto* tracing_dock = create_tracing_dock(this);
    addDockWidget(Qt::BottomDockWidgetArea, tracing_dock);
    auto* terminal_dock = create_terminal_dock(this, controller_);
    addDockWidget(Qt::BottomDockWidgetArea, terminal_dock);
    tabifyDockWidget(tracing_dock, terminal_dock);
    window_menu->addSeparator();
    window_menu->addAction(tracing_dock->toggleViewAction());
    window_menu->addAction(terminal_dock->toggleViewAction());
    resizeDocks({ tracing_dock, terminal_dock }, { 320, 320 }, Qt::Vertical);
    terminal_dock->raise();

    QSettings settings;
    if (settings.value(QStringLiteral("ui/schema"), 0).toInt() == ui_settings_schema) {
      restoreGeometry(settings.value(QStringLiteral("ui/geometry")).toByteArray());
      restoreState(settings.value(QStringLiteral("ui/windowState")).toByteArray());
    }

    auto* about_action = help_menu->addAction(translate(about_text));
    connect(about_action, &QAction::triggered, this, [this] {
      QMessageBox::about(
        this,
        translate(about_text),
        QStringLiteral("%1 %2").arg(
          QCoreApplication::applicationName(),
          QCoreApplication::applicationVersion()));
    });

    auto* state_timer = new QTimer(this);
    state_timer->setInterval(50);
    connect(state_timer, &QTimer::timeout, this, [this] {
      update_emulation_state();
    });
    state_timer->start();
    refresh_action_icons();
    update_emulation_state();
  }

protected:
  void closeEvent(QCloseEvent* event) override
  {
    QSettings settings;
    settings.setValue(QStringLiteral("ui/schema"), ui_settings_schema);
    settings.setValue(QStringLiteral("ui/geometry"), saveGeometry());
    settings.setValue(QStringLiteral("ui/windowState"), saveState());
    settings.sync();
    QMainWindow::closeEvent(event);
  }

  void changeEvent(QEvent* event) override
  {
    QMainWindow::changeEvent(event);
    if (event->type() == QEvent::PaletteChange
        || event->type() == QEvent::ApplicationPaletteChange
        || event->type() == QEvent::StyleChange) {
      refresh_action_icons();
    }
  }

private:
  void refresh_action_icons()
  {
    if (run_pause_action_ == nullptr) {
      return;
    }
    run_pause_action_->setIcon(palette_icon(
      run_icon_running_ ? pause_icon_path : run_icon_path,
      palette()));
    hard_reset_action_->setIcon(palette_icon(hard_reset_icon_path, palette()));
    emulation_settings_action_->setIcon(
      palette_icon(emulation_settings_icon_path, palette()));
  }

  void update_emulation_state()
  {
    const auto snapshot = controller_.snapshot();
    const auto running = snapshot.state == EmulationState::Running;
    run_pause_action_->setText(translate(running ? pause_text : run_text));
    if (run_icon_running_ != running) {
      run_icon_running_ = running;
      run_pause_action_->setIcon(palette_icon(
        running ? pause_icon_path : run_icon_path,
        palette()));
    }
    run_pause_action_->setEnabled(
      running || snapshot.state == EmulationState::Paused);

    hard_reset_action_->setEnabled(
      snapshot.has_machine
      && (snapshot.state == EmulationState::Paused
          || snapshot.state == EmulationState::Running
          || snapshot.state == EmulationState::Idle
          || snapshot.state == EmulationState::Faulted));
    save_state_action_->setEnabled(
      snapshot.has_machine
      && (snapshot.state == EmulationState::Paused
          || snapshot.state == EmulationState::Running
          || snapshot.state == EmulationState::Idle));
    load_state_action_->setEnabled(
      snapshot.state == EmulationState::Unconfigured
      || snapshot.state == EmulationState::Paused
      || snapshot.state == EmulationState::Running
      || snapshot.state == EmulationState::Idle
      || snapshot.state == EmulationState::Faulted);
    emulation_settings_action_->setEnabled(
      snapshot.state == EmulationState::Unconfigured
      || snapshot.state == EmulationState::Paused
      || snapshot.state == EmulationState::Idle
      || snapshot.state == EmulationState::Faulted);
    emulation_status_->setText(
      translate(ip32_status_text).arg(emulation_state_text(snapshot.state)));

    if (snapshot.error_id > last_error_id_ && !snapshot.error_message.empty()) {
      last_error_id_ = snapshot.error_id;
      QMessageBox::critical(
        this,
        translate(emulation_error_text),
        from_rust_string(snapshot.error_message));
    }

    if (snapshot.persistence_id > last_persistence_id_) {
      last_persistence_id_ = snapshot.persistence_id;
      const auto detail = from_rust_string(snapshot.persistence_message);
      switch (snapshot.persistence_outcome) {
      case PersistenceOutcome::Saved:
        statusBar()->showMessage(translate(state_saved_text), 5000);
        break;
      case PersistenceOutcome::Loaded:
        statusBar()->showMessage(translate(state_loaded_text), 5000);
        break;
      case PersistenceOutcome::PromRequired:
        retry_load_with_prom(detail);
        break;
      case PersistenceOutcome::Warning:
        QMessageBox::warning(
          this,
          translate(persistence_warning_text),
          detail);
        break;
      case PersistenceOutcome::Failed:
        QMessageBox::critical(
          this,
          translate(persistence_failed_text),
          detail);
        break;
      case PersistenceOutcome::None:
        break;
      }
    }
  }

  void retry_load_with_prom(const QString& detail)
  {
    QMessageBox::information(
      this,
      translate(load_state_title_text),
      QStringLiteral("%1\n\n%2").arg(translate(select_matching_prom_text), detail));
    QSettings settings;
    const auto prom = QFileDialog::getOpenFileName(
      this,
      translate(select_prom_text),
      settings.value(QStringLiteral("recent/promDirectory")).toString(),
      translate(prom_filter_text));
    if (prom.isEmpty() || pending_load_path_.isEmpty()) {
      return;
    }
    const auto absolute_prom = QFileInfo(prom).absoluteFilePath();
    const auto state_utf8 = pending_load_path_.toUtf8();
    const auto prom_utf8 = absolute_prom.toUtf8();
    if (controller_.request_load_state(
          rust::Str(state_utf8.constData(), static_cast<std::size_t>(state_utf8.size())),
          rust::Str(prom_utf8.constData(), static_cast<std::size_t>(prom_utf8.size())))) {
      settings.setValue(
        QStringLiteral("recent/promDirectory"),
        QFileInfo(absolute_prom).absolutePath());
    }
  }

  const EmulationController& controller_;
  QAction* run_pause_action_ = nullptr;
  QAction* hard_reset_action_ = nullptr;
  QAction* save_state_action_ = nullptr;
  QAction* load_state_action_ = nullptr;
  QAction* emulation_settings_action_ = nullptr;
  QLabel* emulation_status_ = nullptr;
  bool run_icon_running_ = false;
  std::uint64_t last_error_id_ = 0;
  std::uint64_t last_persistence_id_ = 0;
  QString pending_load_path_;
};

std::vector<QByteArray> make_argument_storage(
  const rust::Vec<rust::String>& arguments)
{
  std::vector<QByteArray> storage;
  storage.reserve(arguments.size());

  for (const auto& argument : arguments) {
    storage.emplace_back(
      argument.data(),
      static_cast<qsizetype>(argument.size()));
  }

  if (storage.empty()) {
    storage.emplace_back("sgi-emu");
  }

  return storage;
}

std::vector<char*> make_argument_pointers(std::vector<QByteArray>& storage)
{
  std::vector<char*> pointers;
  pointers.reserve(storage.size() + 1);

  for (auto& argument : storage) {
    pointers.push_back(argument.data());
  }
  pointers.push_back(nullptr);

  return pointers;
}

}

std::int32_t run_application(
  rust::Str version,
  rust::Vec<rust::String> arguments,
  const EmulationController& controller)
{
  initialize_resources();

  auto argument_storage = make_argument_storage(arguments);
  auto argument_pointers = make_argument_pointers(argument_storage);
  auto argument_count = static_cast<int>(argument_storage.size());
  QApplication application(argument_count, argument_pointers.data());

  QCoreApplication::setApplicationName(QStringLiteral("sgi-emu"));
  QCoreApplication::setOrganizationName(QStringLiteral("rickgcn"));
  QCoreApplication::setOrganizationDomain(QStringLiteral("rickgcn"));
  QApplication::setApplicationDisplayName(QStringLiteral("sgi-emu"));
  QCoreApplication::setApplicationVersion(QString::fromUtf8(
    version.data(),
    static_cast<qsizetype>(version.size())));
  QLocale::setDefault(QLocale(QStringLiteral("en_US")));

  QSettings settings;
  const auto saved_style = settings.value(QStringLiteral("ui/style")).toString();
  if (!saved_style.isEmpty()) {
    QApplication::setStyle(saved_style);
  }

  QTranslator translator;
  if (!translator.load(QStringLiteral(":/i18n/sgi-emu_en_US.qm"))) {
    qFatal("Failed to load the en_US translation catalog");
  }
  QCoreApplication::installTranslator(&translator);

  MainWindow main_window(controller);
  main_window.show();

  const auto exit_code = application.exec();
  QCoreApplication::removeTranslator(&translator);
  return exit_code;
}

}
