#include "se_ui/include/display_dock.h"

#include "se_ui/src/application.rs.h"

#include <cstdint>
#include <limits>
#include <utility>

#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QTimer>
#include <QtGui/QCloseEvent>
#include <QtGui/QHideEvent>
#include <QtGui/QImage>
#include <QtGui/QKeyEvent>
#include <QtGui/QPainter>
#include <QtGui/QPaintEvent>
#include <QtGui/QShowEvent>
#include <QtWidgets/QDockWidget>
#include <QtWidgets/QMainWindow>
#include <QtWidgets/QWidget>

namespace se::ui {
namespace {

constexpr int display_poll_interval_ms = 16;
constexpr auto display_text = QT_TRANSLATE_NOOP("DisplayDock", "Display");
constexpr auto no_signal_text =
  QT_TRANSLATE_NOOP("DisplayDock", "No Signal");

QString translate(const char* source)
{
  return QCoreApplication::translate("DisplayDock", source);
}

bool valid_frame_update(const UiDisplayUpdate& update)
{
  if (!update.has_frame
      || update.width == 0
      || update.height == 0
      || update.width > static_cast<std::uint32_t>(std::numeric_limits<int>::max())
      || update.height > static_cast<std::uint32_t>(std::numeric_limits<int>::max())
      || update.stride > static_cast<std::uint32_t>(std::numeric_limits<int>::max())) {
    return false;
  }
  const auto row_bytes = static_cast<std::uint64_t>(update.width) * 4;
  const auto required_bytes =
    static_cast<std::uint64_t>(update.stride) * update.height;
  return update.stride >= row_bytes
      && required_bytes <= update.rgba.size();
}

class DisplayDock final : public QDockWidget
{
public:
  explicit DisplayDock(QMainWindow* parent)
    : QDockWidget(translate(display_text), parent)
    , main_window_(parent)
  {
  }

  void toggleFullScreen()
  {
    if (isFullScreen()) {
      leaveFullScreen();
      return;
    }

    was_floating_ = isFloating();
    dock_area_ = main_window_->dockWidgetArea(this);
    if (was_floating_) {
      geometry_before_fullscreen_ = saveGeometry();
    } else {
      setFloating(true);
    }
    showFullScreen();
  }

  void leaveFullScreen()
  {
    if (!isFullScreen()) {
      return;
    }

    showNormal();
    if (was_floating_) {
      if (!geometry_before_fullscreen_.isEmpty()) {
        restoreGeometry(geometry_before_fullscreen_);
      }
    } else {
      main_window_->addDockWidget(dock_area_, this);
      setFloating(false);
    }
  }

protected:
  void closeEvent(QCloseEvent* event) override
  {
    leaveFullScreen();
    QDockWidget::closeEvent(event);
  }

private:
  QMainWindow* main_window_;
  QByteArray geometry_before_fullscreen_;
  Qt::DockWidgetArea dock_area_ = Qt::RightDockWidgetArea;
  bool was_floating_ = false;
};

class DisplayView final : public QWidget
{
public:
  DisplayView(
    const EmulationController& controller,
    DisplayDock& dock,
    QWidget* parent)
    : QWidget(parent)
    , controller_(controller)
    , dock_(dock)
    , timer_(new QTimer(this))
  {
    setAttribute(Qt::WA_OpaquePaintEvent);
    setFocusPolicy(Qt::StrongFocus);
    timer_->setInterval(display_poll_interval_ms);
    timer_->setTimerType(Qt::PreciseTimer);
    connect(timer_, &QTimer::timeout, this, [this] { pollDisplay(); });
  }

  QSize sizeHint() const override
  {
    return { 640, 480 };
  }

protected:
  void paintEvent(QPaintEvent* event) override
  {
    Q_UNUSED(event);
    QPainter painter(this);
    painter.fillRect(rect(), Qt::black);
    if (image_.isNull()) {
      painter.setPen(Qt::lightGray);
      painter.drawText(rect(), Qt::AlignCenter, translate(no_signal_text));
      return;
    }

    auto target_size = image_.size();
    target_size.scale(size(), Qt::KeepAspectRatio);
    const QRect target(
      (width() - target_size.width()) / 2,
      (height() - target_size.height()) / 2,
      target_size.width(),
      target_size.height());
    painter.setRenderHint(QPainter::SmoothPixmapTransform, false);
    painter.drawImage(target, image_);
  }

  void showEvent(QShowEvent* event) override
  {
    QWidget::showEvent(event);
    pollDisplay();
    timer_->start();
  }

  void hideEvent(QHideEvent* event) override
  {
    timer_->stop();
    QWidget::hideEvent(event);
  }

  void keyPressEvent(QKeyEvent* event) override
  {
    if (event->key() == Qt::Key_F11) {
      dock_.toggleFullScreen();
      event->accept();
      return;
    }
    if (event->key() == Qt::Key_Escape && dock_.isFullScreen()) {
      dock_.leaveFullScreen();
      event->accept();
      return;
    }
    QWidget::keyPressEvent(event);
  }

private:
  void clearFrame()
  {
    if (image_.isNull()) {
      return;
    }
    image_ = {};
    update();
  }

  bool setFrame(const UiDisplayUpdate& display_update)
  {
    if (!valid_frame_update(display_update)) {
      return false;
    }
    const QImage borrowed(
      display_update.rgba.data(),
      static_cast<int>(display_update.width),
      static_cast<int>(display_update.height),
      static_cast<int>(display_update.stride),
      QImage::Format_RGBA8888);
    auto image = borrowed.copy();
    if (image.isNull()) {
      return false;
    }
    image_ = std::move(image);
    update();
    return true;
  }

  void pollDisplay()
  {
    auto display_update = controller_.take_display_update();
    const auto identity_changed = !identity_initialized_
      || display_update.generation != generation_
      || display_update.session_id != session_id_;
    if (identity_changed) {
      identity_initialized_ = true;
      generation_ = display_update.generation;
      session_id_ = display_update.session_id;
      clearFrame();
    }
    if (display_update.has_frame) {
      setFrame(display_update);
    }
  }

  const EmulationController& controller_;
  DisplayDock& dock_;
  QTimer* timer_;
  QImage image_;
  std::uint64_t generation_ = 0;
  std::uint64_t session_id_ = 0;
  bool identity_initialized_ = false;
};

}

QDockWidget* create_display_dock(
  QMainWindow* parent,
  const EmulationController& controller)
{
  auto* dock = new DisplayDock(parent);
  dock->setObjectName(QStringLiteral("displayDock"));
  dock->setAllowedAreas(Qt::AllDockWidgetAreas);
  dock->setFeatures(
    QDockWidget::DockWidgetClosable | QDockWidget::DockWidgetMovable
    | QDockWidget::DockWidgetFloatable);
  dock->setWidget(new DisplayView(controller, *dock, dock));
  return dock;
}

}
