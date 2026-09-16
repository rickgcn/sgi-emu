#include "host_mouse_capture_p.h"

#include <QGuiApplication>
#include <QSocketNotifier>

#include <X11/Xlib.h>
#include <X11/extensions/XInput2.h>

#include <memory>
#include <utility>

namespace se_ui::frontend {
namespace {

class XInput2MouseCapture final : public HostMouseCaptureBackend {
public:
    explicit XInput2MouseCapture(RelativeMotionHandler motion_handler)
        : motion_handler_(std::move(motion_handler)) {
        initialize();
    }

    ~XInput2MouseCapture() override {
        release();
        notifier_.reset();
        if (raw_display_ != nullptr) {
            XCloseDisplay(raw_display_);
        }
    }

    bool capture(QWindow* target) override {
        release();
        if (!available_ || target == nullptr) {
            return false;
        }

        const auto window = static_cast<Window>(target->winId());
        Window root = None;
        Window child = None;
        int root_x = 0;
        int root_y = 0;
        int window_x = 0;
        int window_y = 0;
        unsigned int mask = 0;
        if (window == None
            || XQueryPointer(
                   qt_display_,
                   window,
                   &root,
                   &child,
                   &root_x,
                   &root_y,
                   &window_x,
                   &window_y,
                   &mask)
                == False) {
            return false;
        }

        window_ = window;
        anchor_x_ = window_x;
        anchor_y_ = window_y;
        captured_ = true;
        return true;
    }

    void release() override {
        if (!captured_) {
            return;
        }
        captured_ = false;
        window_ = None;
    }

    bool captured() const override {
        return captured_;
    }

private:
    void initialize() {
        auto* native = qGuiApp->nativeInterface<QNativeInterface::QX11Application>();
        if (native == nullptr || native->display() == nullptr) {
            return;
        }
        qt_display_ = native->display();
        raw_display_ = XOpenDisplay(DisplayString(qt_display_));
        if (raw_display_ == nullptr) {
            return;
        }

        int event = 0;
        int error = 0;
        if (!XQueryExtension(
                raw_display_,
                "XInputExtension",
                &xi_opcode_,
                &event,
                &error)) {
            return;
        }
        int major = 2;
        int minor = 0;
        if (XIQueryVersion(raw_display_, &major, &minor) != Success) {
            return;
        }

        unsigned char mask_bytes[XIMaskLen(XI_RawMotion)]{};
        XISetMask(mask_bytes, XI_RawMotion);
        XIEventMask mask{
            XIAllMasterDevices,
            static_cast<int>(sizeof(mask_bytes)),
            mask_bytes,
        };
        if (XISelectEvents(
                raw_display_, DefaultRootWindow(raw_display_), &mask, 1)
            != Success) {
            return;
        }
        XFlush(raw_display_);

        notifier_ = std::make_unique<QSocketNotifier>(
            ConnectionNumber(raw_display_), QSocketNotifier::Read);
        QObject::connect(
            notifier_.get(),
            &QSocketNotifier::activated,
            [this](auto...) { process_events(); });
        available_ = true;
    }

    void process_events() {
        while (raw_display_ != nullptr && XPending(raw_display_) != 0) {
            XEvent event{};
            XNextEvent(raw_display_, &event);
            auto& cookie = event.xcookie;
            if (cookie.type != GenericEvent || cookie.extension != xi_opcode_
                || !XGetEventData(raw_display_, &cookie)) {
                continue;
            }

            if (captured_ && cookie.evtype == XI_RawMotion) {
                handle_motion(*static_cast<XIRawEvent*>(cookie.data));
                pin_pointer();
            }
            XFreeEventData(raw_display_, &cookie);
        }
    }

    void handle_motion(const XIRawEvent& event) const {
        double delta_x = 0.0;
        double delta_y = 0.0;
        int value_index = 0;
        const int axes = event.valuators.mask_len * 8;
        for (int axis = 0; axis < axes; ++axis) {
            if (!XIMaskIsSet(event.valuators.mask, axis)) {
                continue;
            }
            const double value = event.raw_values[value_index++];
            if (axis == 0) {
                delta_x = value;
            } else if (axis == 1) {
                delta_y = value;
            }
        }
        if (delta_x != 0.0 || delta_y != 0.0) {
            motion_handler_(delta_x, delta_y);
        }
    }

    void pin_pointer() const {
        if (window_ == None) {
            return;
        }
        XWarpPointer(
            qt_display_,
            None,
            window_,
            0,
            0,
            0,
            0,
            anchor_x_,
            anchor_y_);
        XFlush(qt_display_);
    }

    RelativeMotionHandler motion_handler_;
    Display* qt_display_ = nullptr;
    Display* raw_display_ = nullptr;
    std::unique_ptr<QSocketNotifier> notifier_;
    Window window_ = None;
    int xi_opcode_ = 0;
    int anchor_x_ = 0;
    int anchor_y_ = 0;
    bool available_ = false;
    bool captured_ = false;
};

} // namespace

std::unique_ptr<HostMouseCaptureBackend> make_xinput2_mouse_capture(
    RelativeMotionHandler motion_handler) {
    return std::make_unique<XInput2MouseCapture>(std::move(motion_handler));
}

} // namespace se_ui::frontend
