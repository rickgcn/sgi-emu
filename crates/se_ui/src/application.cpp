#include "se_ui/include/application.h"

#include <cstdint>
#include <vector>

#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QEvent>
#include <QtCore/QFile>
#include <QtCore/QLocale>
#include <QtCore/QSignalBlocker>
#include <QtCore/QSize>
#include <QtCore/QString>
#include <QtCore/QTimer>
#include <QtCore/QTranslator>
#include <QtCore/QtGlobal>
#include <QtCore/QtResource>
#include <QtGui/QAction>
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

constexpr auto run_icon_path = ":/icons/run.svg";
constexpr auto pause_icon_path = ":/icons/pause.svg";
constexpr auto hard_reset_icon_path = ":/icons/hard-reset.svg";
constexpr auto emulation_settings_icon_path =
  ":/icons/emulation-settings.svg";

QString translate(const char* source)
{
  return QCoreApplication::translate("MainWindow", source);
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
  }
}

class MainWindow final : public QMainWindow
{
public:
  MainWindow()
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
        run_icon_running_ = !run_icon_running_;
        run_pause_action_->setText(
          translate(run_icon_running_ ? pause_text : run_text));
        refresh_action_icons();
      });

    hard_reset_action_ = action_menu->addAction(translate(hard_reset_text));
    auto* settings_action = tools_menu->addAction(translate(settings_text));
    connect(settings_action, &QAction::triggered, this, [this] {
      show_settings_dialog(this);
    });
    tools_menu->addSeparator();
    emulation_settings_action_ =
      tools_menu->addAction(translate(emulation_settings_text));

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

    auto* about_action = help_menu->addAction(translate(about_text));
    connect(about_action, &QAction::triggered, this, [this] {
      QMessageBox::about(
        this,
        translate(about_text),
        QStringLiteral("%1 %2").arg(
          QCoreApplication::applicationName(),
          QCoreApplication::applicationVersion()));
    });

    refresh_action_icons();
  }

protected:
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

  QAction* run_pause_action_ = nullptr;
  QAction* hard_reset_action_ = nullptr;
  QAction* emulation_settings_action_ = nullptr;
  bool run_icon_running_ = false;
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
  rust::Vec<rust::String> arguments)
{
  initialize_resources();

  auto argument_storage = make_argument_storage(arguments);
  auto argument_pointers = make_argument_pointers(argument_storage);
  auto argument_count = static_cast<int>(argument_storage.size());
  QApplication application(argument_count, argument_pointers.data());

  QCoreApplication::setApplicationName(QStringLiteral("sgi-emu"));
  QApplication::setApplicationDisplayName(QStringLiteral("sgi-emu"));
  QCoreApplication::setApplicationVersion(QString::fromUtf8(
    version.data(),
    static_cast<qsizetype>(version.size())));
  QLocale::setDefault(QLocale(QStringLiteral("en_US")));

  QTranslator translator;
  if (!translator.load(QStringLiteral(":/i18n/sgi-emu_en_US.qm"))) {
    qFatal("Failed to load the en_US translation catalog");
  }
  QCoreApplication::installTranslator(&translator);

  MainWindow main_window;
  main_window.show();

  const auto exit_code = application.exec();
  QCoreApplication::removeTranslator(&translator);
  return exit_code;
}

}
