#include "se_ui/display_widget.h"

#include "se_ui/frontend/host_mouse_capture.h"
#include "se_ui/src/bridge.rs.h"

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
#include <map>
#include <optional>
#include <unordered_map>
#include <utility>

namespace se_ui {
namespace {

KeyboardKeyDto named(KeyboardNamedKeyDto key) {
    return {KeyboardKeyKindDto::Named, static_cast<std::uint8_t>(key)};
}

KeyboardKeyDto letter(char key) {
    return {KeyboardKeyKindDto::Letter, static_cast<std::uint8_t>(key)};
}

KeyboardKeyDto digit(std::uint8_t key) {
    return {KeyboardKeyKindDto::Digit, key};
}

KeyboardKeyDto keypad_digit(std::uint8_t key) {
    return {KeyboardKeyKindDto::KeypadDigit, key};
}

std::optional<KeyboardKeyDto> keypad_key(const QKeyEvent& event) {
    if (!event.modifiers().testFlag(Qt::KeypadModifier)) {
        return std::nullopt;
    }
    switch (event.key()) {
    case Qt::Key_0: case Qt::Key_Insert: return keypad_digit(0);
    case Qt::Key_1: case Qt::Key_End: return keypad_digit(1);
    case Qt::Key_2: case Qt::Key_Down: return keypad_digit(2);
    case Qt::Key_3: case Qt::Key_PageDown: return keypad_digit(3);
    case Qt::Key_4: case Qt::Key_Left: return keypad_digit(4);
    case Qt::Key_5: case Qt::Key_Clear: return keypad_digit(5);
    case Qt::Key_6: case Qt::Key_Right: return keypad_digit(6);
    case Qt::Key_7: case Qt::Key_Home: return keypad_digit(7);
    case Qt::Key_8: case Qt::Key_Up: return keypad_digit(8);
    case Qt::Key_9: case Qt::Key_PageUp: return keypad_digit(9);
    case Qt::Key_Period: case Qt::Key_Comma: case Qt::Key_Delete:
        return named(KeyboardNamedKeyDto::KeypadPeriod);
    case Qt::Key_Slash: return named(KeyboardNamedKeyDto::KeypadSlash);
    case Qt::Key_Asterisk: return named(KeyboardNamedKeyDto::KeypadAsterisk);
    case Qt::Key_Minus: return named(KeyboardNamedKeyDto::KeypadMinus);
    case Qt::Key_Plus: return named(KeyboardNamedKeyDto::KeypadPlus);
    case Qt::Key_Enter: case Qt::Key_Return:
        return named(KeyboardNamedKeyDto::KeypadEnter);
    default: return std::nullopt;
    }
}

std::optional<KeyboardKeyDto> mapped_key(const QKeyEvent& event) {
    if (const auto keypad = keypad_key(event); keypad.has_value()) {
        return keypad;
    }
    if (event.key() >= Qt::Key_A && event.key() <= Qt::Key_Z) {
        return letter(static_cast<char>(event.key()));
    }
    if (event.key() >= Qt::Key_0 && event.key() <= Qt::Key_9) {
        return digit(static_cast<std::uint8_t>(event.key() - Qt::Key_0));
    }
    if (event.key() >= Qt::Key_F1 && event.key() <= Qt::Key_F12) {
        return KeyboardKeyDto{KeyboardKeyKindDto::Function,
                              static_cast<std::uint8_t>(event.key() - Qt::Key_F1 + 1)};
    }
    using K = KeyboardNamedKeyDto;
    switch (event.key()) {
    case Qt::Key_Shift: return named(K::LeftShift);
#ifdef Q_OS_MACOS
    case Qt::Key_Meta: return named(K::LeftControl);
    case Qt::Key_Control: return std::nullopt;
#else
    case Qt::Key_Control: return named(K::LeftControl);
#endif
    case Qt::Key_Alt: return named(K::LeftAlt);
    case Qt::Key_AltGr: return named(K::RightAlt);
    case Qt::Key_CapsLock: return named(K::CapsLock);
    case Qt::Key_Escape: return named(K::Escape);
    case Qt::Key_Tab: case Qt::Key_Backtab: return named(K::Tab);
    case Qt::Key_Return: return named(K::Enter);
    case Qt::Key_Backspace: return named(K::Backspace);
    case Qt::Key_Delete: return named(K::Delete);
    case Qt::Key_Space: return named(K::Space);
    case Qt::Key_Left: return named(K::ArrowLeft);
    case Qt::Key_Right: return named(K::ArrowRight);
    case Qt::Key_Up: return named(K::ArrowUp);
    case Qt::Key_Down: return named(K::ArrowDown);
    case Qt::Key_Insert: return named(K::Insert);
    case Qt::Key_Home: return named(K::Home);
    case Qt::Key_End: return named(K::End);
    case Qt::Key_PageUp: return named(K::PageUp);
    case Qt::Key_PageDown: return named(K::PageDown);
    case Qt::Key_Print: case Qt::Key_SysReq: return named(K::PrintScreen);
    case Qt::Key_ScrollLock: return named(K::ScrollLock);
    case Qt::Key_Pause: return named(K::Pause);
    case Qt::Key_NumLock: return named(K::NumLock);
    case Qt::Key_Exclam: return digit(1);
    case Qt::Key_At: return digit(2);
    case Qt::Key_NumberSign: return digit(3);
    case Qt::Key_Dollar: return digit(4);
    case Qt::Key_Percent: return digit(5);
    case Qt::Key_AsciiCircum: return digit(6);
    case Qt::Key_Ampersand: return digit(7);
    case Qt::Key_Asterisk: return digit(8);
    case Qt::Key_ParenLeft: return digit(9);
    case Qt::Key_ParenRight: return digit(0);
    case Qt::Key_Semicolon: case Qt::Key_Colon: return named(K::Semicolon);
    case Qt::Key_Comma: case Qt::Key_Less: return named(K::Comma);
    case Qt::Key_Minus: case Qt::Key_Underscore: return named(K::Minus);
    case Qt::Key_BracketLeft: case Qt::Key_BraceLeft: return named(K::LeftBracket);
    case Qt::Key_BracketRight: case Qt::Key_BraceRight: return named(K::RightBracket);
    case Qt::Key_Apostrophe: case Qt::Key_QuoteDbl: return named(K::Apostrophe);
    case Qt::Key_Period: case Qt::Key_Greater: return named(K::Period);
    case Qt::Key_Slash: case Qt::Key_Question: return named(K::Slash);
    case Qt::Key_Equal: case Qt::Key_Plus: return named(K::Equal);
    case Qt::Key_QuoteLeft: case Qt::Key_AsciiTilde: return named(K::Grave);
    case Qt::Key_Backslash: case Qt::Key_Bar: return named(K::Backslash);
    default: return std::nullopt;
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

std::uint16_t key_identity(KeyboardKeyDto key) {
    return static_cast<std::uint16_t>(static_cast<std::uint8_t>(key.kind)) << 8 | key.value;
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
    VideoOutputStateDto output = VideoOutputStateDto::NoSignal;
    std::optional<rust::Box<VideoFrameHandle>> frame;
    QImage image;
    bool input_enabled = false;
    bool captured = false;
    bool capture_click_armed = false;
    bool motion_delivery_scheduled = false;
    std::unordered_map<std::uint32_t, KeyboardKeyDto> scan_keys;
    std::map<std::uint16_t, std::uint32_t> key_counts;
    std::optional<EndpointIdentity> keyboard_endpoint;
    std::optional<EndpointIdentity> pointer_endpoint;
    std::array<bool, 3> mouse_buttons{};
    std::int64_t pending_x = 0;
    std::int64_t pending_y = 0;
    double fractional_x = 0.0;
    double fractional_y = 0.0;
    std::unique_ptr<frontend::HostMouseCapture> mouse_capture;
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
    state_->mouse_capture = std::make_unique<frontend::HostMouseCapture>(
        [this](double delta_x, double delta_y) {
            handle_host_motion(delta_x, delta_y);
        });
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

void DisplayWidget::set_input_endpoints(std::optional<EndpointIdentity> keyboard, std::optional<EndpointIdentity> pointer) {
    release_guest_inputs();
    state_->keyboard_endpoint = std::move(keyboard);
    state_->pointer_endpoint = std::move(pointer);
}

void DisplayWidget::release_input() {
    release_guest_inputs();
}

bool DisplayWidget::event(QEvent* event) {
    if (event->type() == QEvent::UngrabMouse && state_->captured) {
        release_guest_inputs();
        event->accept();
        return true;
    }
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
    if (!state_->input_enabled || !state_->keyboard_endpoint.has_value() || event->isAutoRepeat()) {
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
            auto& count = state_->key_counts[key_identity(*key)];
            if (count++ == 0) {
                if (!drain_motion() || !send_keyboard(*key, true)) {
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
            auto& count = state_->key_counts[key_identity(saved_key)];
            if (count != 0 && --count == 0) {
                if (!drain_motion() || !send_keyboard(saved_key, false)) {
                    abort_input();
                }
            }
        }
    } else {
        auto& count = state_->key_counts[key_identity(*key)];
        if (pressed) {
            if (count++ == 0) {
                if (!drain_motion() || !send_keyboard(*key, true)) {
                    abort_input();
                }
            }
        } else if (count != 0 && --count == 0) {
            if (!drain_motion() || !send_keyboard(*key, false)) {
                abort_input();
            }
        }
    }
    event->accept();
}

void DisplayWidget::mousePressEvent(QMouseEvent* event) {
    if (!state_->input_enabled || !state_->pointer_endpoint.has_value()) {
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
    PointerButtonDto button = PointerButtonDto::Left;
    if (event->button() == Qt::LeftButton) {
        index = 0;
    } else if (event->button() == Qt::MiddleButton) {
        index = 1;
        button = PointerButtonDto::Middle;
    } else if (event->button() == Qt::RightButton) {
        index = 2;
        button = PointerButtonDto::Right;
    }
    if (index.has_value() && !state_->mouse_buttons[*index]) {
        if (!drain_motion() || !send_pointer_button(button, true)) {
            abort_input();
        } else {
            state_->mouse_buttons[*index] = true;
        }
    }
    event->accept();
}

void DisplayWidget::mouseReleaseEvent(QMouseEvent* event) {
    if (!state_->input_enabled || !state_->pointer_endpoint.has_value()) {
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
    PointerButtonDto button = PointerButtonDto::Left;
    if (event->button() == Qt::LeftButton) {
        index = 0;
    } else if (event->button() == Qt::MiddleButton) {
        index = 1;
        button = PointerButtonDto::Middle;
    } else if (event->button() == Qt::RightButton) {
        index = 2;
        button = PointerButtonDto::Right;
    }
    if (index.has_value() && state_->mouse_buttons[*index]) {
        if (!drain_motion() || !send_pointer_button(button, false)) {
            abort_input();
        } else {
            state_->mouse_buttons[*index] = false;
        }
    }
    event->accept();
}

void DisplayWidget::mouseMoveEvent(QMouseEvent* event) {
    event->accept();
}

void DisplayWidget::begin_pointer_capture() {
    if (state_->captured || !state_->input_enabled || !state_->pointer_endpoint.has_value()) {
        return;
    }
    state_->capture_click_armed = false;
    state_->pending_x = 0;
    state_->pending_y = 0;
    state_->fractional_x = 0;
    state_->fractional_y = 0;
    state_->mouse_buttons.fill(false);
    if (!state_->mouse_capture->capture(this)) {
        return;
    }
    state_->captured = true;
    setFocus(Qt::MouseFocusReason);

    for (const auto button : {
             PointerButtonDto::Left,
             PointerButtonDto::Middle,
             PointerButtonDto::Right,
         }) {
        if (!send_pointer_button(button, false)) {
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
    state_->mouse_capture->release();
}

void DisplayWidget::abort_input() {
    state_->scan_keys.clear();
    state_->key_counts.clear();
    state_->mouse_buttons.fill(false);
    end_pointer_capture();
}

void DisplayWidget::release_guest_inputs() {
    if (!drain_motion()) {
        abort_input();
        return;
    }
    for (const auto& [identity, count] : state_->key_counts) {
        const KeyboardKeyDto key{
            static_cast<KeyboardKeyKindDto>(identity >> 8),
            static_cast<std::uint8_t>(identity & 0xff),
        };
        if (count != 0 && !send_keyboard(key, false)) {
            abort_input();
            return;
        }
    }
    const std::array<PointerButtonDto, 3> buttons{
        PointerButtonDto::Left,
        PointerButtonDto::Middle,
        PointerButtonDto::Right,
    };
    for (std::size_t index = 0; index < buttons.size(); ++index) {
        if (state_->mouse_buttons[index]
            && !send_pointer_button(buttons[index], false)) {
            abort_input();
            return;
        }
    }
    state_->scan_keys.clear();
    state_->key_counts.clear();
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
        if (!send_pointer_motion(delta_x, delta_y)) {
            return false;
        }
        state_->pending_x -= delta_x;
        state_->pending_y -= delta_y;
    }
    return true;
}

bool DisplayWidget::send_keyboard(const KeyboardKeyDto& key, bool pressed) const {
    if (!state_->keyboard_endpoint.has_value()) {
        return true;
    }
    const auto handle = endpoint_handle_dto(*state_->keyboard_endpoint);
    return state_->session.send_keyboard(handle, key, pressed);
}

bool DisplayWidget::send_pointer_motion(std::int32_t delta_x, std::int32_t delta_y) const {
    if (!state_->pointer_endpoint.has_value()) {
        return true;
    }
    const auto handle = endpoint_handle_dto(*state_->pointer_endpoint);
    return state_->session.send_pointer_motion(handle, delta_x, delta_y);
}

bool DisplayWidget::send_pointer_button(PointerButtonDto button, bool pressed) const {
    if (!state_->pointer_endpoint.has_value()) {
        return true;
    }
    const auto handle = endpoint_handle_dto(*state_->pointer_endpoint);
    return state_->session.send_pointer_button(handle, button, pressed);
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
    if (state_->output == VideoOutputStateDto::NoSignal) {
        message = QStringLiteral("No signal");
    }
    if (!message.isEmpty()) {
        painter.setPen(Qt::white);
        painter.drawText(rect(), Qt::AlignCenter, message);
    }
}

} // namespace se_ui
