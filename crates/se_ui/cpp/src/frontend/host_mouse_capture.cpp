#include "se_ui/frontend/host_mouse_capture.h"

#include "host_mouse_capture_p.h"

#include <QCursor>
#include <QGuiApplication>
#include <QMetaObject>
#include <QPointer>
#include <QString>

#include <utility>

namespace se_ui::frontend {
namespace {

class UnsupportedMouseCapture final : public HostMouseCaptureBackend {
public:
    bool capture(QWindow*) override {
        return false;
    }

    void release() override {
    }

    bool captured() const override {
        return false;
    }
};

std::unique_ptr<HostMouseCaptureBackend> make_backend(
    RelativeMotionHandler motion_handler) {
    const auto platform = QGuiApplication::platformName().toStdString();
    const auto kind = select_host_mouse_backend(platform);

#if defined(Q_OS_WINDOWS)
    if (kind == HostMouseBackendKind::WindowsRawInput) {
        return make_windows_mouse_capture(std::move(motion_handler));
    }
#elif defined(Q_OS_LINUX)
    if (kind == HostMouseBackendKind::Wayland) {
        return make_wayland_mouse_capture(std::move(motion_handler));
    }
    if (kind == HostMouseBackendKind::XInput2) {
        return make_xinput2_mouse_capture(std::move(motion_handler));
    }
#elif defined(Q_OS_MACOS)
    if (kind == HostMouseBackendKind::MacOs) {
        return make_macos_mouse_capture(std::move(motion_handler));
    }
#endif

    return std::make_unique<UnsupportedMouseCapture>();
}

} // namespace

struct HostMouseCapture::Implementation {
    explicit Implementation(RelativeMotionHandler motion_handler)
        : backend(make_backend(std::move(motion_handler))) {
    }

    bool capture(QWindow* capture_target) {
        release();
        if (capture_target == nullptr || capture_target->handle() == nullptr
            || !capture_target->isVisible()
            || !backend->capture(capture_target)) {
            return false;
        }

        target = capture_target;
        QGuiApplication::setOverrideCursor(Qt::BlankCursor);
        cursor_override_active = true;
        target_destroyed = QObject::connect(
            capture_target,
            &QObject::destroyed,
            [this] {
                backend->release();
                restore_cursor();
                target = nullptr;
                target_destroyed = {};
            });
        return true;
    }

    void release() {
        if (target_destroyed) {
            QObject::disconnect(target_destroyed);
            target_destroyed = {};
        }
        backend->release();
        restore_cursor();
        target = nullptr;
    }

    void restore_cursor() {
        if (cursor_override_active && QGuiApplication::instance() != nullptr) {
            QGuiApplication::restoreOverrideCursor();
        }
        cursor_override_active = false;
    }

    std::unique_ptr<HostMouseCaptureBackend> backend;
    QPointer<QWindow> target;
    QMetaObject::Connection target_destroyed;
    bool cursor_override_active = false;
};

HostMouseCapture::HostMouseCapture(RelativeMotionHandler motion_handler)
    : implementation_(
          std::make_unique<Implementation>(std::move(motion_handler))) {
}

HostMouseCapture::~HostMouseCapture() {
    implementation_->release();
}

bool HostMouseCapture::capture(QWindow* target) {
    return implementation_->capture(target);
}

void HostMouseCapture::release() {
    implementation_->release();
}

bool HostMouseCapture::captured() const {
    return implementation_->backend->captured();
}

} // namespace se_ui::frontend
