#include "se_ui/include/display_dock.h"

#include "se_ui/src/application.rs.h"

#include <algorithm>
#include <cstdint>
#include <limits>
#include <optional>
#include <set>
#include <utility>

#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QTimer>
#include <QtCore/QtGlobal>
#include <QtGui/QCloseEvent>
#include <QtGui/QCursor>
#include <QtGui/QFocusEvent>
#include <QtGui/QGuiApplication>
#include <QtGui/QHideEvent>
#include <QtGui/QImage>
#include <QtGui/QKeyEvent>
#include <QtGui/QMouseEvent>
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

bool is_control_key(UiPhysicalKey key)
{
  return key == UiPhysicalKey::LeftControl
      || key == UiPhysicalKey::RightControl;
}

bool is_alt_key(UiPhysicalKey key)
{
  return key == UiPhysicalKey::LeftAlt
      || key == UiPhysicalKey::RightAlt;
}

int saturated_negate(int value)
{
  return value == std::numeric_limits<int>::min()
    ? std::numeric_limits<int>::max()
    : -value;
}

#ifdef Q_OS_MACOS
std::optional<UiPhysicalKey> mac_physical_key(const QKeyEvent& event)
{
  switch (event.nativeVirtualKey()) {
  case 0x00: return UiPhysicalKey::A;
  case 0x01: return UiPhysicalKey::S;
  case 0x02: return UiPhysicalKey::D;
  case 0x03: return UiPhysicalKey::F;
  case 0x04: return UiPhysicalKey::H;
  case 0x05: return UiPhysicalKey::G;
  case 0x06: return UiPhysicalKey::Z;
  case 0x07: return UiPhysicalKey::X;
  case 0x08: return UiPhysicalKey::C;
  case 0x09: return UiPhysicalKey::V;
  case 0x0a: return UiPhysicalKey::Iso102;
  case 0x0b: return UiPhysicalKey::B;
  case 0x0c: return UiPhysicalKey::Q;
  case 0x0d: return UiPhysicalKey::W;
  case 0x0e: return UiPhysicalKey::E;
  case 0x0f: return UiPhysicalKey::R;
  case 0x10: return UiPhysicalKey::Y;
  case 0x11: return UiPhysicalKey::T;
  case 0x12: return UiPhysicalKey::Digit1;
  case 0x13: return UiPhysicalKey::Digit2;
  case 0x14: return UiPhysicalKey::Digit3;
  case 0x15: return UiPhysicalKey::Digit4;
  case 0x16: return UiPhysicalKey::Digit6;
  case 0x17: return UiPhysicalKey::Digit5;
  case 0x18: return UiPhysicalKey::Equal;
  case 0x19: return UiPhysicalKey::Digit9;
  case 0x1a: return UiPhysicalKey::Digit7;
  case 0x1b: return UiPhysicalKey::Minus;
  case 0x1c: return UiPhysicalKey::Digit8;
  case 0x1d: return UiPhysicalKey::Digit0;
  case 0x1e: return UiPhysicalKey::RightBracket;
  case 0x1f: return UiPhysicalKey::O;
  case 0x20: return UiPhysicalKey::U;
  case 0x21: return UiPhysicalKey::LeftBracket;
  case 0x22: return UiPhysicalKey::I;
  case 0x23: return UiPhysicalKey::P;
  case 0x24: return UiPhysicalKey::Enter;
  case 0x25: return UiPhysicalKey::L;
  case 0x26: return UiPhysicalKey::J;
  case 0x27: return UiPhysicalKey::Apostrophe;
  case 0x28: return UiPhysicalKey::K;
  case 0x29: return UiPhysicalKey::Semicolon;
  case 0x2a:
    return event.key() == Qt::Key_NumberSign
      ? UiPhysicalKey::IsoHash
      : UiPhysicalKey::Backslash;
  case 0x2b: return UiPhysicalKey::Comma;
  case 0x2c: return UiPhysicalKey::Slash;
  case 0x2d: return UiPhysicalKey::N;
  case 0x2e: return UiPhysicalKey::M;
  case 0x2f: return UiPhysicalKey::Period;
  case 0x30: return UiPhysicalKey::Tab;
  case 0x31: return UiPhysicalKey::Space;
  case 0x32: return UiPhysicalKey::Grave;
  case 0x33: return UiPhysicalKey::Backspace;
  case 0x35: return UiPhysicalKey::Escape;
  case 0x36:
  case 0x37:
  case 0x3f:
    return std::nullopt;
  case 0x38: return UiPhysicalKey::LeftShift;
  case 0x39: return UiPhysicalKey::CapsLock;
  case 0x3a: return UiPhysicalKey::LeftAlt;
  case 0x3b: return UiPhysicalKey::LeftControl;
  case 0x3c: return UiPhysicalKey::RightShift;
  case 0x3d: return UiPhysicalKey::RightAlt;
  case 0x3e: return UiPhysicalKey::RightControl;
  case 0x41: return UiPhysicalKey::NumpadDecimal;
  case 0x43: return UiPhysicalKey::NumpadMultiply;
  case 0x45: return UiPhysicalKey::NumpadAdd;
  case 0x47: return UiPhysicalKey::NumLock;
  case 0x4b: return UiPhysicalKey::NumpadDivide;
  case 0x4c: return UiPhysicalKey::NumpadEnter;
  case 0x4e: return UiPhysicalKey::NumpadSubtract;
  case 0x52: return UiPhysicalKey::Numpad0;
  case 0x53: return UiPhysicalKey::Numpad1;
  case 0x54: return UiPhysicalKey::Numpad2;
  case 0x55: return UiPhysicalKey::Numpad3;
  case 0x56: return UiPhysicalKey::Numpad4;
  case 0x57: return UiPhysicalKey::Numpad5;
  case 0x58: return UiPhysicalKey::Numpad6;
  case 0x59: return UiPhysicalKey::Numpad7;
  case 0x5b: return UiPhysicalKey::Numpad8;
  case 0x5c: return UiPhysicalKey::Numpad9;
  case 0x60: return UiPhysicalKey::F5;
  case 0x61: return UiPhysicalKey::F6;
  case 0x62: return UiPhysicalKey::F7;
  case 0x63: return UiPhysicalKey::F3;
  case 0x64: return UiPhysicalKey::F8;
  case 0x65: return UiPhysicalKey::F9;
  case 0x67: return UiPhysicalKey::F11;
  case 0x69: return UiPhysicalKey::PrintScreen;
  case 0x6b: return UiPhysicalKey::ScrollLock;
  case 0x6d: return UiPhysicalKey::F10;
  case 0x6f: return UiPhysicalKey::F12;
  case 0x71: return UiPhysicalKey::Pause;
  case 0x72: return UiPhysicalKey::Insert;
  case 0x73: return UiPhysicalKey::Home;
  case 0x74: return UiPhysicalKey::PageUp;
  case 0x75: return UiPhysicalKey::Delete;
  case 0x76: return UiPhysicalKey::F4;
  case 0x77: return UiPhysicalKey::End;
  case 0x78: return UiPhysicalKey::F2;
  case 0x79: return UiPhysicalKey::PageDown;
  case 0x7a: return UiPhysicalKey::F1;
  case 0x7b: return UiPhysicalKey::ArrowLeft;
  case 0x7c: return UiPhysicalKey::ArrowRight;
  case 0x7d: return UiPhysicalKey::ArrowDown;
  case 0x7e: return UiPhysicalKey::ArrowUp;
  default: return std::nullopt;
  }
}
#endif

#ifndef Q_OS_MACOS
std::optional<UiPhysicalKey> portable_physical_key(const QKeyEvent& event)
{
  const auto keypad = event.modifiers().testFlag(Qt::KeypadModifier);
  if (keypad) {
    switch (event.key()) {
    case Qt::Key_0: return UiPhysicalKey::Numpad0;
    case Qt::Key_1: return UiPhysicalKey::Numpad1;
    case Qt::Key_2: return UiPhysicalKey::Numpad2;
    case Qt::Key_3: return UiPhysicalKey::Numpad3;
    case Qt::Key_4: return UiPhysicalKey::Numpad4;
    case Qt::Key_5: return UiPhysicalKey::Numpad5;
    case Qt::Key_6: return UiPhysicalKey::Numpad6;
    case Qt::Key_7: return UiPhysicalKey::Numpad7;
    case Qt::Key_8: return UiPhysicalKey::Numpad8;
    case Qt::Key_9: return UiPhysicalKey::Numpad9;
    case Qt::Key_Enter: return UiPhysicalKey::NumpadEnter;
    case Qt::Key_Plus: return UiPhysicalKey::NumpadAdd;
    case Qt::Key_Minus: return UiPhysicalKey::NumpadSubtract;
    case Qt::Key_Asterisk: return UiPhysicalKey::NumpadMultiply;
    case Qt::Key_Slash: return UiPhysicalKey::NumpadDivide;
    case Qt::Key_Period: return UiPhysicalKey::NumpadDecimal;
    default: break;
    }
  }

  switch (event.key()) {
  case Qt::Key_Escape: return UiPhysicalKey::Escape;
  case Qt::Key_F1: return UiPhysicalKey::F1;
  case Qt::Key_F2: return UiPhysicalKey::F2;
  case Qt::Key_F3: return UiPhysicalKey::F3;
  case Qt::Key_F4: return UiPhysicalKey::F4;
  case Qt::Key_F5: return UiPhysicalKey::F5;
  case Qt::Key_F6: return UiPhysicalKey::F6;
  case Qt::Key_F7: return UiPhysicalKey::F7;
  case Qt::Key_F8: return UiPhysicalKey::F8;
  case Qt::Key_F9: return UiPhysicalKey::F9;
  case Qt::Key_F10: return UiPhysicalKey::F10;
  case Qt::Key_F11: return UiPhysicalKey::F11;
  case Qt::Key_F12: return UiPhysicalKey::F12;
  case Qt::Key_Print: return UiPhysicalKey::PrintScreen;
  case Qt::Key_ScrollLock: return UiPhysicalKey::ScrollLock;
  case Qt::Key_Pause: return UiPhysicalKey::Pause;
  case Qt::Key_QuoteLeft: return UiPhysicalKey::Grave;
  case Qt::Key_1: return UiPhysicalKey::Digit1;
  case Qt::Key_2: return UiPhysicalKey::Digit2;
  case Qt::Key_3: return UiPhysicalKey::Digit3;
  case Qt::Key_4: return UiPhysicalKey::Digit4;
  case Qt::Key_5: return UiPhysicalKey::Digit5;
  case Qt::Key_6: return UiPhysicalKey::Digit6;
  case Qt::Key_7: return UiPhysicalKey::Digit7;
  case Qt::Key_8: return UiPhysicalKey::Digit8;
  case Qt::Key_9: return UiPhysicalKey::Digit9;
  case Qt::Key_0: return UiPhysicalKey::Digit0;
  case Qt::Key_Minus: return UiPhysicalKey::Minus;
  case Qt::Key_Equal: return UiPhysicalKey::Equal;
  case Qt::Key_Backspace: return UiPhysicalKey::Backspace;
  case Qt::Key_Insert: return UiPhysicalKey::Insert;
  case Qt::Key_Home: return UiPhysicalKey::Home;
  case Qt::Key_PageUp: return UiPhysicalKey::PageUp;
  case Qt::Key_NumLock: return UiPhysicalKey::NumLock;
  case Qt::Key_Tab: return UiPhysicalKey::Tab;
  case Qt::Key_Q: return UiPhysicalKey::Q;
  case Qt::Key_W: return UiPhysicalKey::W;
  case Qt::Key_E: return UiPhysicalKey::E;
  case Qt::Key_R: return UiPhysicalKey::R;
  case Qt::Key_T: return UiPhysicalKey::T;
  case Qt::Key_Y: return UiPhysicalKey::Y;
  case Qt::Key_U: return UiPhysicalKey::U;
  case Qt::Key_I: return UiPhysicalKey::I;
  case Qt::Key_O: return UiPhysicalKey::O;
  case Qt::Key_P: return UiPhysicalKey::P;
  case Qt::Key_BracketLeft: return UiPhysicalKey::LeftBracket;
  case Qt::Key_BracketRight: return UiPhysicalKey::RightBracket;
  case Qt::Key_Backslash: return UiPhysicalKey::Backslash;
  case Qt::Key_NumberSign: return UiPhysicalKey::IsoHash;
  case Qt::Key_Delete: return UiPhysicalKey::Delete;
  case Qt::Key_End: return UiPhysicalKey::End;
  case Qt::Key_PageDown: return UiPhysicalKey::PageDown;
  case Qt::Key_CapsLock: return UiPhysicalKey::CapsLock;
  case Qt::Key_A: return UiPhysicalKey::A;
  case Qt::Key_S: return UiPhysicalKey::S;
  case Qt::Key_D: return UiPhysicalKey::D;
  case Qt::Key_F: return UiPhysicalKey::F;
  case Qt::Key_G: return UiPhysicalKey::G;
  case Qt::Key_H: return UiPhysicalKey::H;
  case Qt::Key_J: return UiPhysicalKey::J;
  case Qt::Key_K: return UiPhysicalKey::K;
  case Qt::Key_L: return UiPhysicalKey::L;
  case Qt::Key_Semicolon: return UiPhysicalKey::Semicolon;
  case Qt::Key_Apostrophe: return UiPhysicalKey::Apostrophe;
  case Qt::Key_Return: return UiPhysicalKey::Enter;
  case Qt::Key_Shift: return UiPhysicalKey::LeftShift;
  case Qt::Key_Less: return UiPhysicalKey::Iso102;
  case Qt::Key_Z: return UiPhysicalKey::Z;
  case Qt::Key_X: return UiPhysicalKey::X;
  case Qt::Key_C: return UiPhysicalKey::C;
  case Qt::Key_V: return UiPhysicalKey::V;
  case Qt::Key_B: return UiPhysicalKey::B;
  case Qt::Key_N: return UiPhysicalKey::N;
  case Qt::Key_M: return UiPhysicalKey::M;
  case Qt::Key_Comma: return UiPhysicalKey::Comma;
  case Qt::Key_Period: return UiPhysicalKey::Period;
  case Qt::Key_Slash: return UiPhysicalKey::Slash;
  case Qt::Key_Up: return UiPhysicalKey::ArrowUp;
  case Qt::Key_Control: return UiPhysicalKey::LeftControl;
  case Qt::Key_Alt: return UiPhysicalKey::LeftAlt;
  case Qt::Key_Space: return UiPhysicalKey::Space;
  case Qt::Key_Left: return UiPhysicalKey::ArrowLeft;
  case Qt::Key_Down: return UiPhysicalKey::ArrowDown;
  case Qt::Key_Right: return UiPhysicalKey::ArrowRight;
  case Qt::Key_Meta:
  default: return std::nullopt;
  }
}
#endif

std::optional<UiPhysicalKey> physical_key(const QKeyEvent& event)
{
#ifdef Q_OS_MACOS
  return mac_physical_key(event);
#else
  return portable_physical_key(event);
#endif
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
    setMouseTracking(true);
    timer_->setInterval(display_poll_interval_ms);
    timer_->setTimerType(Qt::PreciseTimer);
    connect(timer_, &QTimer::timeout, this, [this] { pollDisplay(); });
    connect(
      qGuiApp,
      &QGuiApplication::applicationStateChanged,
      this,
      [this](Qt::ApplicationState state) {
        if (state != Qt::ApplicationActive) {
          releaseCapture();
        }
      });
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
    releaseCapture();
    QWidget::hideEvent(event);
  }

  void focusOutEvent(QFocusEvent* event) override
  {
    releaseCapture();
    QWidget::focusOutEvent(event);
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
    submitKeyEvent(*event, true);
  }

  void keyReleaseEvent(QKeyEvent* event) override
  {
    if (event->key() == Qt::Key_F11) {
      event->accept();
      return;
    }
    if (event->key() == Qt::Key_Escape && dock_.isFullScreen()) {
      event->accept();
      return;
    }
    submitKeyEvent(*event, false);
  }

  void mousePressEvent(QMouseEvent* event) override
  {
    if (!captured_) {
      beginCapture();
      event->accept();
      return;
    }
    submitMouseButtons(event->buttons());
    event->accept();
  }

  void mouseReleaseEvent(QMouseEvent* event) override
  {
    if (!captured_) {
      QWidget::mouseReleaseEvent(event);
      return;
    }
    submitMouseButtons(event->buttons());
    event->accept();
  }

  void mouseMoveEvent(QMouseEvent* event) override
  {
    if (!captured_) {
      QWidget::mouseMoveEvent(event);
      return;
    }
    QPoint delta;
    if (warp_mouse_) {
      const auto center = rect().center();
      delta = event->position().toPoint() - center;
      if (!delta.isNull()) {
        QCursor::setPos(mapToGlobal(center));
      }
    } else {
      const auto position = event->position().toPoint();
      delta = position - last_mouse_position_;
      last_mouse_position_ = position;
    }
    if (!delta.isNull()) {
      const auto status = controller_.submit_mouse_input(
        delta.x(),
        saturated_negate(delta.y()),
        mouse_buttons_);
      if (status == UiInputStatus::Unavailable) {
        releaseCapture();
      }
    }
    event->accept();
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
    const auto snapshot = controller_.snapshot();
    if (!snapshot.has_machine
        || snapshot.state == EmulationState::Faulted
        || snapshot.state == EmulationState::ShuttingDown) {
      releaseCapture();
    }
    auto display_update = controller_.take_display_update();
    const auto identity_changed = !identity_initialized_
      || display_update.generation != generation_
      || display_update.session_id != session_id_;
    if (identity_changed) {
      releaseCapture();
      identity_initialized_ = true;
      generation_ = display_update.generation;
      session_id_ = display_update.session_id;
      clearFrame();
    }
    if (display_update.has_frame) {
      setFrame(display_update);
    }
  }

  void beginCapture()
  {
    if (captured_) {
      return;
    }
    setFocus(Qt::MouseFocusReason);
    if (controller_.release_all_input() == UiInputStatus::Unavailable) {
      return;
    }
    pressed_keys_.clear();
    mouse_buttons_ = {};
    grabKeyboard();
    grabMouse();
    if (QWidget::keyboardGrabber() != this || QWidget::mouseGrabber() != this) {
      releaseKeyboard();
      releaseMouse();
      return;
    }
    captured_ = true;
    setCursor(Qt::BlankCursor);
    warp_mouse_ = !QGuiApplication::platformName().contains(
      QStringLiteral("wayland"),
      Qt::CaseInsensitive);
    last_mouse_position_ = mapFromGlobal(QCursor::pos());
    if (warp_mouse_) {
      QCursor::setPos(mapToGlobal(rect().center()));
    }
  }

  void releaseCapture()
  {
    if (!captured_ && pressed_keys_.empty()
        && !mouse_buttons_.left && !mouse_buttons_.middle
        && !mouse_buttons_.right) {
      return;
    }
    controller_.release_all_input();
    pressed_keys_.clear();
    mouse_buttons_ = {};
    if (QWidget::keyboardGrabber() == this) {
      releaseKeyboard();
    }
    if (QWidget::mouseGrabber() == this) {
      releaseMouse();
    }
    unsetCursor();
    captured_ = false;
  }

  void submitKeyEvent(QKeyEvent& event, bool pressed)
  {
    if (!captured_ || event.isAutoRepeat()) {
      event.accept();
      return;
    }
    const auto key = physical_key(event);
    if (!key.has_value()) {
      event.accept();
      return;
    }
    if (pressed) {
      if (!pressed_keys_.insert(*key).second) {
        event.accept();
        return;
      }
      const auto control_down = std::any_of(
        pressed_keys_.begin(),
        pressed_keys_.end(),
        is_control_key);
      const auto alt_down = std::any_of(
        pressed_keys_.begin(),
        pressed_keys_.end(),
        is_alt_key);
      if (control_down && alt_down) {
        releaseCapture();
        event.accept();
        return;
      }
    } else {
      if (pressed_keys_.erase(*key) == 0) {
        event.accept();
        return;
      }
    }
    if (controller_.submit_key_input(*key, pressed)
        == UiInputStatus::Unavailable) {
      releaseCapture();
    }
    event.accept();
  }

  void submitMouseButtons(Qt::MouseButtons buttons)
  {
    mouse_buttons_.left = buttons.testFlag(Qt::LeftButton);
    mouse_buttons_.middle = buttons.testFlag(Qt::MiddleButton);
    mouse_buttons_.right = buttons.testFlag(Qt::RightButton);
    if (controller_.submit_mouse_input(0, 0, mouse_buttons_)
        == UiInputStatus::Unavailable) {
      releaseCapture();
    }
  }

  const EmulationController& controller_;
  DisplayDock& dock_;
  QTimer* timer_;
  QImage image_;
  std::uint64_t generation_ = 0;
  std::uint64_t session_id_ = 0;
  bool identity_initialized_ = false;
  bool captured_ = false;
  bool warp_mouse_ = false;
  QPoint last_mouse_position_;
  UiMouseButtons mouse_buttons_ {};
  std::set<UiPhysicalKey> pressed_keys_;
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
