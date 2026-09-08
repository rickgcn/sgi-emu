#include "se_ui/display_widget.h"

#include "se_ui/src/bridge.rs.h"

#include <QCursor>
#include <QEvent>
#include <QFocusEvent>
#include <QHideEvent>
#include <QImage>
#include <QKeyEvent>
#include <QMetaObject>
#include <QMouseEvent>
#include <QPainter>
#include <QPaintEvent>

#include <algorithm>
#include <array>
#include <cmath>
#include <limits>
#include <optional>
#include <unordered_map>
#include <utility>

namespace se_ui {
namespace {

std::optional<std::uint8_t> keypad_key(const QKeyEvent& event) {
    if (!event.modifiers().testFlag(Qt::KeypadModifier)) {
        return std::nullopt;
    }
    switch (event.key()) {
    case Qt::Key_0:
    case Qt::Key_Insert:
        return 58;
    case Qt::Key_1:
    case Qt::Key_End:
        return 57;
    case Qt::Key_2:
    case Qt::Key_Down:
        return 63;
    case Qt::Key_3:
    case Qt::Key_PageDown:
        return 64;
    case Qt::Key_4:
    case Qt::Key_Left:
        return 62;
    case Qt::Key_5:
    case Qt::Key_Clear:
        return 68;
    case Qt::Key_6:
    case Qt::Key_Right:
        return 69;
    case Qt::Key_7:
    case Qt::Key_Home:
        return 66;
    case Qt::Key_8:
    case Qt::Key_Up:
        return 67;
    case Qt::Key_9:
    case Qt::Key_PageUp:
        return 74;
    case Qt::Key_Period:
    case Qt::Key_Comma:
    case Qt::Key_Delete:
        return 65;
    case Qt::Key_Slash:
        return 107;
    case Qt::Key_Asterisk:
        return 108;
    case Qt::Key_Minus:
        return 75;
    case Qt::Key_Plus:
        return 109;
    case Qt::Key_Enter:
    case Qt::Key_Return:
        return 81;
    default:
        return std::nullopt;
    }
}

std::optional<std::uint8_t> mapped_key(const QKeyEvent& event) {
    if (const auto keypad = keypad_key(event); keypad.has_value()) {
        return keypad;
    }
    switch (event.key()) {
    case Qt::Key_Shift:
        return 5;
#ifdef Q_OS_MACOS
    case Qt::Key_Meta:
        return 2;
    case Qt::Key_Control:
        return std::nullopt;
#else
    case Qt::Key_Control:
        return 2;
#endif
    case Qt::Key_Alt:
        return 83;
    case Qt::Key_AltGr:
        return 84;
    case Qt::Key_CapsLock:
        return 3;
    case Qt::Key_Escape:
        return 6;
    case Qt::Key_1:
    case Qt::Key_Exclam:
        return 7;
    case Qt::Key_Tab:
    case Qt::Key_Backtab:
        return 8;
    case Qt::Key_Q:
        return 9;
    case Qt::Key_A:
        return 10;
    case Qt::Key_S:
        return 11;
    case Qt::Key_2:
    case Qt::Key_At:
        return 13;
    case Qt::Key_3:
    case Qt::Key_NumberSign:
        return 14;
    case Qt::Key_W:
        return 15;
    case Qt::Key_E:
        return 16;
    case Qt::Key_D:
        return 17;
    case Qt::Key_F:
        return 18;
    case Qt::Key_Z:
        return 19;
    case Qt::Key_X:
        return 20;
    case Qt::Key_4:
    case Qt::Key_Dollar:
        return 21;
    case Qt::Key_5:
    case Qt::Key_Percent:
        return 22;
    case Qt::Key_R:
        return 23;
    case Qt::Key_T:
        return 24;
    case Qt::Key_G:
        return 25;
    case Qt::Key_H:
        return 26;
    case Qt::Key_C:
        return 27;
    case Qt::Key_V:
        return 28;
    case Qt::Key_6:
    case Qt::Key_AsciiCircum:
        return 29;
    case Qt::Key_7:
    case Qt::Key_Ampersand:
        return 30;
    case Qt::Key_Y:
        return 31;
    case Qt::Key_U:
        return 32;
    case Qt::Key_J:
        return 33;
    case Qt::Key_K:
        return 34;
    case Qt::Key_B:
        return 35;
    case Qt::Key_N:
        return 36;
    case Qt::Key_8:
    case Qt::Key_Asterisk:
        return 37;
    case Qt::Key_9:
    case Qt::Key_ParenLeft:
        return 38;
    case Qt::Key_I:
        return 39;
    case Qt::Key_O:
        return 40;
    case Qt::Key_L:
        return 41;
    case Qt::Key_Semicolon:
    case Qt::Key_Colon:
        return 42;
    case Qt::Key_M:
        return 43;
    case Qt::Key_Comma:
    case Qt::Key_Less:
        return 44;
    case Qt::Key_0:
    case Qt::Key_ParenRight:
        return 45;
    case Qt::Key_Minus:
    case Qt::Key_Underscore:
        return 46;
    case Qt::Key_P:
        return 47;
    case Qt::Key_BracketLeft:
    case Qt::Key_BraceLeft:
        return 48;
    case Qt::Key_Apostrophe:
    case Qt::Key_QuoteDbl:
        return 49;
    case Qt::Key_Return:
        return 50;
    case Qt::Key_Period:
    case Qt::Key_Greater:
        return 51;
    case Qt::Key_Slash:
    case Qt::Key_Question:
        return 52;
    case Qt::Key_Equal:
    case Qt::Key_Plus:
        return 53;
    case Qt::Key_QuoteLeft:
    case Qt::Key_AsciiTilde:
        return 54;
    case Qt::Key_BracketRight:
    case Qt::Key_BraceRight:
        return 55;
    case Qt::Key_Backslash:
    case Qt::Key_Bar:
        return 56;
    case Qt::Key_Backspace:
        return 60;
    case Qt::Key_Delete:
        return 61;
    case Qt::Key_Left:
        return 72;
    case Qt::Key_Down:
        return 73;
    case Qt::Key_Right:
        return 79;
    case Qt::Key_Up:
        return 80;
    case Qt::Key_Space:
        return 82;
    case Qt::Key_F1:
        return 86;
    case Qt::Key_F2:
        return 87;
    case Qt::Key_F3:
        return 88;
    case Qt::Key_F4:
        return 89;
    case Qt::Key_F5:
        return 90;
    case Qt::Key_F6:
        return 91;
    case Qt::Key_F7:
        return 92;
    case Qt::Key_F8:
        return 93;
    case Qt::Key_F9:
        return 94;
    case Qt::Key_F10:
        return 95;
    case Qt::Key_F11:
        return 96;
    case Qt::Key_F12:
        return 97;
    case Qt::Key_Print:
    case Qt::Key_SysReq:
        return 98;
    case Qt::Key_ScrollLock:
        return 99;
    case Qt::Key_Pause:
        return 100;
    case Qt::Key_Insert:
        return 101;
    case Qt::Key_Home:
        return 102;
    case Qt::Key_PageUp:
        return 103;
    case Qt::Key_End:
        return 104;
    case Qt::Key_PageDown:
        return 105;
    case Qt::Key_NumLock:
        return 106;
    default:
        return std::nullopt;
    }
}

bool release_chord(const QKeyEvent& event) {
    if (event.key() != Qt::Key_G || !event.modifiers().testFlag(Qt::AltModifier)) {
        return false;
    }
#ifdef Q_OS_MACOS
    return event.modifiers().testFlag(Qt::MetaModifier);
#else
    return event.modifiers().testFlag(Qt::ControlModifier);
#endif
}

std::int32_t clamp_i32(std::int64_t value) {
    return static_cast<std::int32_t>(std::clamp(
        value,
        std::int64_t(std::numeric_limits<std::int32_t>::min()),
        std::int64_t(std::numeric_limits<std::int32_t>::max())));
}

} // namespace

struct DisplayWidget::State {
    explicit State(const UiSession& session_value)
        : session(session_value) {
    }

    const UiSession& session;
    VideoOutputStateDto output = VideoOutputStateDto::NoGraphicsBoard;
    std::optional<rust::Box<VideoFrameHandle>> frame;
    QImage image;
    bool input_enabled = false;
    bool captured = false;
    bool capture_click_armed = false;
    bool motion_delivery_scheduled = false;
    std::unordered_map<std::uint32_t, std::uint8_t> scan_keys;
    std::array<std::uint32_t, 128> key_counts{};
    std::array<bool, 3> mouse_buttons{};
    std::int64_t pending_x = 0;
    std::int64_t pending_y = 0;
    double fractional_x = 0.0;
    double fractional_y = 0.0;
};

DisplayWidget::DisplayWidget(const UiSession& session, QWidget* parent)
    : QWidget(parent)
    , state_(std::make_unique<State>(session)) {
    setObjectName(QStringLiteral("DisplayWidget"));
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
    setAttribute(Qt::WA_OpaquePaintEvent);
    setAttribute(Qt::WA_KeyCompression, false);
    setFocusPolicy(Qt::StrongFocus);
    setMouseTracking(true);
}

DisplayWidget::~DisplayWidget() {
    abort_input();
}

void DisplayWidget::set_video_output(
    VideoOutputStateDto output,
    rust::Box<VideoFrameHandle> frame) {
    state_->image = QImage();
    state_->frame.reset();
    state_->output = output;

    if (output == VideoOutputStateDto::Frame) {
        state_->frame.emplace(std::move(frame));
        const auto& retained = **state_->frame;
        const auto pixels = retained.pixels();
        const auto width = retained.width();
        const auto height = retained.height();
        state_->image = QImage(
            pixels.data(),
            static_cast<int>(width),
            static_cast<int>(height),
            static_cast<qsizetype>(width) * 4,
            QImage::Format_RGBA8888);
        if (state_->image.isNull()) {
            state_->frame.reset();
            state_->output = VideoOutputStateDto::Blank;
        }
    }

    update();
}

void DisplayWidget::set_input_enabled(bool enabled) {
    if (state_->input_enabled == enabled) {
        return;
    }
    if (!enabled) {
        release_guest_inputs();
    }
    state_->input_enabled = enabled;
}

void DisplayWidget::release_input() {
    release_guest_inputs();
}

bool DisplayWidget::event(QEvent* event) {
    if (event->type() == QEvent::ShortcutOverride && state_->input_enabled) {
        const auto* key_event = static_cast<QKeyEvent*>(event);
        if (release_chord(*key_event) || mapped_key(*key_event).has_value()) {
            event->accept();
            return true;
        }
    }
    if (event->type() == QEvent::KeyPress || event->type() == QEvent::KeyRelease) {
        const auto* key_event = static_cast<QKeyEvent*>(event);
        if (key_event->key() == Qt::Key_Tab || key_event->key() == Qt::Key_Backtab) {
            handle_key(static_cast<QKeyEvent*>(event), event->type() == QEvent::KeyPress);
            return true;
        }
    }
    return QWidget::event(event);
}

void DisplayWidget::focusOutEvent(QFocusEvent* event) {
    release_guest_inputs();
    QWidget::focusOutEvent(event);
}

void DisplayWidget::hideEvent(QHideEvent* event) {
    release_guest_inputs();
    QWidget::hideEvent(event);
}

void DisplayWidget::keyPressEvent(QKeyEvent* event) {
    handle_key(event, true);
}

void DisplayWidget::keyReleaseEvent(QKeyEvent* event) {
    handle_key(event, false);
}

void DisplayWidget::handle_key(QKeyEvent* event, bool pressed) {
    if (!state_->input_enabled || event->isAutoRepeat()) {
        event->accept();
        return;
    }
    if (pressed && state_->captured && release_chord(*event)) {
        release_guest_inputs();
        event->accept();
        return;
    }
    const auto key = mapped_key(*event);
    if (!key.has_value()) {
        event->ignore();
        return;
    }

    const auto scan_code = event->nativeScanCode();
    if (scan_code != 0) {
        if (pressed) {
            const auto [_, inserted] = state_->scan_keys.emplace(scan_code, *key);
            if (!inserted) {
                event->accept();
                return;
            }
            auto& count = state_->key_counts[*key];
            if (count++ == 0) {
                if (!drain_motion() || !state_->session.send_sgi_key(*key, true)) {
                    abort_input();
                }
            }
        } else {
            const auto found = state_->scan_keys.find(scan_code);
            if (found == state_->scan_keys.end()) {
                event->accept();
                return;
            }
            const auto saved_key = found->second;
            state_->scan_keys.erase(found);
            auto& count = state_->key_counts[saved_key];
            if (count != 0 && --count == 0) {
                if (!drain_motion() || !state_->session.send_sgi_key(saved_key, false)) {
                    abort_input();
                }
            }
        }
    } else {
        auto& count = state_->key_counts[*key];
        if (pressed) {
            if (count++ == 0) {
                if (!drain_motion() || !state_->session.send_sgi_key(*key, true)) {
                    abort_input();
                }
            }
        } else if (count != 0 && --count == 0) {
            if (!drain_motion() || !state_->session.send_sgi_key(*key, false)) {
                abort_input();
            }
        }
    }
    event->accept();
}

void DisplayWidget::mousePressEvent(QMouseEvent* event) {
    if (!state_->input_enabled) {
        event->ignore();
        return;
    }
    if (!state_->captured) {
        state_->capture_click_armed = event->button() == Qt::LeftButton
            && event->buttons() == Qt::LeftButton && rect().contains(event->position().toPoint());
        event->accept();
        return;
    }

    std::optional<std::size_t> index;
    SgiMouseButtonDto button = SgiMouseButtonDto::Left;
    if (event->button() == Qt::LeftButton) {
        index = 0;
    } else if (event->button() == Qt::MiddleButton) {
        index = 1;
        button = SgiMouseButtonDto::Middle;
    } else if (event->button() == Qt::RightButton) {
        index = 2;
        button = SgiMouseButtonDto::Right;
    }
    if (index.has_value() && !state_->mouse_buttons[*index]) {
        if (!drain_motion() || !state_->session.send_sgi_mouse_button(button, true)) {
            abort_input();
        } else {
            state_->mouse_buttons[*index] = true;
        }
    }
    event->accept();
}

void DisplayWidget::mouseReleaseEvent(QMouseEvent* event) {
    if (!state_->input_enabled) {
        event->ignore();
        return;
    }
    if (!state_->captured) {
        const bool capture = state_->capture_click_armed && event->button() == Qt::LeftButton
            && event->buttons() == Qt::NoButton && rect().contains(event->position().toPoint());
        state_->capture_click_armed = false;
        if (capture) {
            begin_pointer_capture();
        }
        event->accept();
        return;
    }

    std::optional<std::size_t> index;
    SgiMouseButtonDto button = SgiMouseButtonDto::Left;
    if (event->button() == Qt::LeftButton) {
        index = 0;
    } else if (event->button() == Qt::MiddleButton) {
        index = 1;
        button = SgiMouseButtonDto::Middle;
    } else if (event->button() == Qt::RightButton) {
        index = 2;
        button = SgiMouseButtonDto::Right;
    }
    if (index.has_value() && state_->mouse_buttons[*index]) {
        if (!drain_motion() || !state_->session.send_sgi_mouse_button(button, false)) {
            abort_input();
        } else {
            state_->mouse_buttons[*index] = false;
        }
    }
    event->accept();
}

void DisplayWidget::mouseMoveEvent(QMouseEvent* event) {
    if (!state_->captured) {
        event->accept();
        return;
    }
    const QPoint center = mapToGlobal(rect().center());
    const QPoint current = event->globalPosition().toPoint();
    if (current != center) {
        handle_host_motion(current.x() - center.x(), current.y() - center.y());
        QCursor::setPos(center);
    }
    event->accept();
}

void DisplayWidget::begin_pointer_capture() {
    if (state_->captured || !state_->input_enabled) {
        return;
    }
    state_->captured = true;
    state_->capture_click_armed = false;
    state_->pending_x = 0;
    state_->pending_y = 0;
    state_->fractional_x = 0;
    state_->fractional_y = 0;
    state_->mouse_buttons.fill(false);
    grabMouse(Qt::BlankCursor);
    setFocus(Qt::MouseFocusReason);
    QCursor::setPos(mapToGlobal(rect().center()));

    for (const auto button : {
             SgiMouseButtonDto::Left,
             SgiMouseButtonDto::Middle,
             SgiMouseButtonDto::Right,
         }) {
        if (!state_->session.send_sgi_mouse_button(button, false)) {
            abort_input();
            return;
        }
    }
}

void DisplayWidget::end_pointer_capture() {
    if (!state_->captured) {
        return;
    }
    state_->captured = false;
    state_->capture_click_armed = false;
    state_->pending_x = 0;
    state_->pending_y = 0;
    state_->fractional_x = 0;
    state_->fractional_y = 0;
    releaseMouse();
    unsetCursor();
}

void DisplayWidget::abort_input() {
    state_->scan_keys.clear();
    state_->key_counts.fill(0);
    state_->mouse_buttons.fill(false);
    end_pointer_capture();
}

void DisplayWidget::release_guest_inputs() {
    if (!drain_motion()) {
        abort_input();
        return;
    }
    for (std::size_t code = 0; code < state_->key_counts.size(); ++code) {
        if (state_->key_counts[code] != 0
            && !state_->session.send_sgi_key(static_cast<std::uint8_t>(code), false)) {
            abort_input();
            return;
        }
    }
    const std::array<SgiMouseButtonDto, 3> buttons{
        SgiMouseButtonDto::Left,
        SgiMouseButtonDto::Middle,
        SgiMouseButtonDto::Right,
    };
    for (std::size_t index = 0; index < buttons.size(); ++index) {
        if (state_->mouse_buttons[index]
            && !state_->session.send_sgi_mouse_button(buttons[index], false)) {
            abort_input();
            return;
        }
    }
    state_->scan_keys.clear();
    state_->key_counts.fill(0);
    state_->mouse_buttons.fill(false);
    end_pointer_capture();
}

void DisplayWidget::handle_host_motion(double delta_x, double delta_y) {
    if (!state_->captured) {
        return;
    }
    const double accumulated_x = delta_x + state_->fractional_x;
    const double accumulated_y = -delta_y + state_->fractional_y;
    const auto integer_x = static_cast<std::int64_t>(std::trunc(accumulated_x));
    const auto integer_y = static_cast<std::int64_t>(std::trunc(accumulated_y));
    state_->fractional_x = accumulated_x - static_cast<double>(integer_x);
    state_->fractional_y = accumulated_y - static_cast<double>(integer_y);
    if (integer_x == 0 && integer_y == 0) {
        return;
    }
    const bool x_overflow = (integer_x > 0
                                && state_->pending_x
                                    > std::numeric_limits<std::int64_t>::max() - integer_x)
        || (integer_x < 0
            && state_->pending_x < std::numeric_limits<std::int64_t>::min() - integer_x);
    const bool y_overflow = (integer_y > 0
                                && state_->pending_y
                                    > std::numeric_limits<std::int64_t>::max() - integer_y)
        || (integer_y < 0
            && state_->pending_y < std::numeric_limits<std::int64_t>::min() - integer_y);
    if ((x_overflow || y_overflow) && !drain_motion()) {
        abort_input();
        return;
    }
    state_->pending_x += integer_x;
    state_->pending_y += integer_y;
    schedule_motion_delivery();
}

void DisplayWidget::schedule_motion_delivery() {
    if (state_->motion_delivery_scheduled) {
        return;
    }
    state_->motion_delivery_scheduled = true;
    if (!QMetaObject::invokeMethod(
            this,
            [this] {
                state_->motion_delivery_scheduled = false;
                if (!drain_motion()) {
                    abort_input();
                }
            },
            Qt::QueuedConnection)) {
        state_->motion_delivery_scheduled = false;
        abort_input();
    }
}

bool DisplayWidget::drain_motion() {
    while (state_->pending_x != 0 || state_->pending_y != 0) {
        const auto delta_x = clamp_i32(state_->pending_x);
        const auto delta_y = clamp_i32(state_->pending_y);
        if (!state_->session.send_sgi_mouse_motion(delta_x, delta_y)) {
            return false;
        }
        state_->pending_x -= delta_x;
        state_->pending_y -= delta_y;
    }
    return true;
}

void DisplayWidget::paintEvent(QPaintEvent* event) {
    event->accept();
    QPainter painter(this);
    painter.fillRect(rect(), Qt::black);

    if (state_->output == VideoOutputStateDto::Frame && !state_->image.isNull()) {
        const auto scaled = state_->image.size().scaled(size(), Qt::KeepAspectRatio);
        QRect destination(QPoint(0, 0), scaled);
        destination.moveCenter(rect().center());
        painter.setRenderHint(QPainter::SmoothPixmapTransform, true);
        painter.drawImage(destination, state_->image);
        return;
    }

    QString message;
    if (state_->output == VideoOutputStateDto::NoGraphicsBoard) {
        message = QStringLiteral("No graphics board");
    } else if (state_->output == VideoOutputStateDto::NoSignal) {
        message = QStringLiteral("No signal");
    }
    if (!message.isEmpty()) {
        painter.setPen(Qt::white);
        painter.drawText(rect(), Qt::AlignCenter, message);
    }
}

} // namespace se_ui
