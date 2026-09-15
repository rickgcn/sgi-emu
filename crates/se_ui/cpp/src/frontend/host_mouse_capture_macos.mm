#include "host_mouse_capture_p.h"

#include <QAbstractNativeEventFilter>
#include <QCoreApplication>

#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>

#include <utility>

namespace se_ui::frontend {
namespace {

class MacOsMouseCapture final : public HostMouseCaptureBackend,
                                public QAbstractNativeEventFilter {
public:
    explicit MacOsMouseCapture(RelativeMotionHandler motion_handler)
        : motion_handler_(std::move(motion_handler)) {
        QCoreApplication::instance()->installNativeEventFilter(this);
    }

    ~MacOsMouseCapture() override {
        release();
        QCoreApplication::instance()->removeNativeEventFilter(this);
    }

    bool capture(QWindow* target) override {
        release();
        if (target == nullptr
            || CGAssociateMouseAndMouseCursorPosition(false)
                != kCGErrorSuccess) {
            return false;
        }
        captured_ = true;
        return true;
    }

    void release() override {
        if (!captured_) {
            return;
        }
        captured_ = false;
        CGAssociateMouseAndMouseCursorPosition(true);
    }

    bool captured() const override {
        return captured_;
    }

    bool nativeEventFilter(
        const QByteArray& event_type,
        void* message,
        qintptr*) override {
        if (!captured_ || event_type != QByteArrayLiteral("mac_generic_NSEvent")
            || message == nullptr) {
            return false;
        }

        NSEvent* event = static_cast<NSEvent*>(message);
        switch ([event type]) {
        case NSEventTypeMouseMoved:
        case NSEventTypeLeftMouseDragged:
        case NSEventTypeRightMouseDragged:
        case NSEventTypeOtherMouseDragged:
            motion_handler_([event deltaX], [event deltaY]);
            break;
        default:
            break;
        }
        return false;
    }

private:
    RelativeMotionHandler motion_handler_;
    bool captured_ = false;
};

} // namespace

std::unique_ptr<HostMouseCaptureBackend> make_macos_mouse_capture(
    RelativeMotionHandler motion_handler) {
    return std::make_unique<MacOsMouseCapture>(std::move(motion_handler));
}

} // namespace se_ui::frontend
