#include "host_mouse_capture_p.h"

#include <QAbstractNativeEventFilter>
#include <QCoreApplication>

#define NOMINMAX
#include <windows.h>

#include <optional>
#include <utility>
#include <vector>

namespace se_ui::frontend {
namespace {

class WindowsMouseCapture final : public HostMouseCaptureBackend,
                                  public QAbstractNativeEventFilter {
public:
    explicit WindowsMouseCapture(RelativeMotionHandler motion_handler)
        : motion_handler_(std::move(motion_handler)) {
        QCoreApplication::instance()->installNativeEventFilter(this);
    }

    ~WindowsMouseCapture() override {
        release();
        QCoreApplication::instance()->removeNativeEventFilter(this);
    }

    bool capture(QWindow* target) override {
        release();
        if (target == nullptr) {
            return false;
        }

        const auto window = reinterpret_cast<HWND>(target->winId());
        RAWINPUTDEVICE device{
            0x01,
            0x02,
            RIDEV_INPUTSINK,
            window,
        };
        if (window == nullptr
            || !RegisterRawInputDevices(&device, 1, sizeof(device))) {
            return false;
        }

        SetCapture(window);
        if (GetCapture() != window || !clip_to_window(window)) {
            if (GetCapture() == window) {
                ReleaseCapture();
            }
            unregister_raw_input();
            return false;
        }

        window_ = window;
        captured_ = true;
        absolute_position_.reset();
        return true;
    }

    void release() override {
        if (!captured_) {
            return;
        }

        captured_ = false;
        ClipCursor(nullptr);
        if (GetCapture() == window_) {
            ReleaseCapture();
        }
        unregister_raw_input();
        absolute_position_.reset();
        window_ = nullptr;
    }

    bool captured() const override {
        return captured_;
    }

    bool nativeEventFilter(
        const QByteArray& event_type,
        void* message,
        qintptr*) override {
        if (!captured_ || event_type != QByteArrayLiteral("windows_generic_MSG")
            || message == nullptr) {
            return false;
        }

        const auto* native_message = static_cast<MSG*>(message);
        if (native_message->message == WM_INPUT) {
            handle_raw_input(reinterpret_cast<HRAWINPUT>(native_message->lParam));
        } else if (native_message->hwnd == window_
                   && native_message->message == WM_WINDOWPOSCHANGED) {
            clip_to_window(window_);
        }
        return false;
    }

private:
    static bool clip_to_window(HWND window) {
        RECT client{};
        if (!GetClientRect(window, &client)) {
            return false;
        }
        POINT corners[2]{
            {client.left, client.top},
            {client.right, client.bottom},
        };
        MapWindowPoints(window, nullptr, corners, 2);
        const RECT screen{
            corners[0].x,
            corners[0].y,
            corners[1].x,
            corners[1].y,
        };
        return ClipCursor(&screen) != FALSE;
    }

    static void unregister_raw_input() {
        RAWINPUTDEVICE device{
            0x01,
            0x02,
            RIDEV_REMOVE,
            nullptr,
        };
        RegisterRawInputDevices(&device, 1, sizeof(device));
    }

    void handle_raw_input(HRAWINPUT handle) {
        UINT size = 0;
        if (GetRawInputData(
                handle,
                RID_INPUT,
                nullptr,
                &size,
                sizeof(RAWINPUTHEADER)) != 0
            || size < sizeof(RAWINPUTHEADER)) {
            return;
        }

        std::vector<BYTE> buffer(size);
        if (GetRawInputData(
                handle,
                RID_INPUT,
                buffer.data(),
                &size,
                sizeof(RAWINPUTHEADER)) != size) {
            return;
        }

        const auto* input = reinterpret_cast<const RAWINPUT*>(buffer.data());
        if (input->header.dwType != RIM_TYPEMOUSE) {
            return;
        }

        const auto& mouse = input->data.mouse;
        if ((mouse.usFlags & MOUSE_MOVE_ABSOLUTE) == 0) {
            if (mouse.lLastX != 0 || mouse.lLastY != 0) {
                motion_handler_(mouse.lLastX, mouse.lLastY);
            }
            absolute_position_.reset();
            return;
        }

        const int width = GetSystemMetrics(
            (mouse.usFlags & MOUSE_VIRTUAL_DESKTOP) != 0
                ? SM_CXVIRTUALSCREEN
                : SM_CXSCREEN);
        const int height = GetSystemMetrics(
            (mouse.usFlags & MOUSE_VIRTUAL_DESKTOP) != 0
                ? SM_CYVIRTUALSCREEN
                : SM_CYSCREEN);
        const POINT position{
            MulDiv(mouse.lLastX, width, 65535),
            MulDiv(mouse.lLastY, height, 65535),
        };
        if (absolute_position_.has_value()) {
            const auto delta_x = position.x - absolute_position_->x;
            const auto delta_y = position.y - absolute_position_->y;
            if (delta_x != 0 || delta_y != 0) {
                motion_handler_(delta_x, delta_y);
            }
        }
        absolute_position_ = position;
    }

    RelativeMotionHandler motion_handler_;
    HWND window_ = nullptr;
    bool captured_ = false;
    std::optional<POINT> absolute_position_;
};

} // namespace

std::unique_ptr<HostMouseCaptureBackend> make_windows_mouse_capture(
    RelativeMotionHandler motion_handler) {
    return std::make_unique<WindowsMouseCapture>(std::move(motion_handler));
}

} // namespace se_ui::frontend
