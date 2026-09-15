#include "host_mouse_capture_p.h"

#include <QGuiApplication>

#include <qpa/qplatformnativeinterface.h>

#include <pointer-constraints-unstable-v1-client-protocol.h>
#include <relative-pointer-unstable-v1-client-protocol.h>
#include <wayland-client-core.h>
#include <wayland-client-protocol.h>

#include <algorithm>
#include <cstring>
#include <utility>

namespace se_ui::frontend {
namespace {

class WaylandMouseCapture final : public HostMouseCaptureBackend {
public:
    explicit WaylandMouseCapture(RelativeMotionHandler motion_handler)
        : motion_handler_(std::move(motion_handler)) {
        initialize();
    }

    ~WaylandMouseCapture() override {
        release();
        if (relative_pointer_manager_ != nullptr) {
            zwp_relative_pointer_manager_v1_destroy(relative_pointer_manager_);
        }
        if (pointer_constraints_ != nullptr) {
            zwp_pointer_constraints_v1_destroy(pointer_constraints_);
        }
        if (registry_ != nullptr) {
            wl_registry_destroy(registry_);
        }
    }

    bool capture(QWindow* target) override {
        release();
        if (display_ == nullptr || relative_pointer_manager_ == nullptr
            || pointer_constraints_ == nullptr || target == nullptr
            || target->handle() == nullptr) {
            return false;
        }

        auto* platform = QGuiApplication::platformNativeInterface();
        if (platform == nullptr
            || platform->nativeResourceForIntegration("wl_display") != display_
            || platform->nativeResourceForIntegration("wl_seat") == nullptr) {
            return false;
        }

        auto* pointer = static_cast<wl_pointer*>(
            platform->nativeResourceForIntegration("wl_pointer"));
        auto* surface = static_cast<wl_surface*>(
            platform->nativeResourceForWindow("surface", target));
        if (pointer == nullptr || surface == nullptr) {
            return false;
        }

        relative_pointer_ =
            zwp_relative_pointer_manager_v1_get_relative_pointer(
                relative_pointer_manager_, pointer);
        if (relative_pointer_ == nullptr) {
            return false;
        }
        zwp_relative_pointer_v1_add_listener(
            relative_pointer_, &relative_pointer_listener_, this);

        locked_pointer_ = zwp_pointer_constraints_v1_lock_pointer(
            pointer_constraints_,
            surface,
            pointer,
            nullptr,
            ZWP_POINTER_CONSTRAINTS_V1_LIFETIME_PERSISTENT);
        if (locked_pointer_ == nullptr) {
            zwp_relative_pointer_v1_destroy(relative_pointer_);
            relative_pointer_ = nullptr;
            return false;
        }
        zwp_locked_pointer_v1_add_listener(
            locked_pointer_, &locked_pointer_listener_, this);

        captured_ = true;
        lock_active_ = false;
        wl_display_flush(display_);
        return true;
    }

    void release() override {
        captured_ = false;
        lock_active_ = false;
        if (locked_pointer_ != nullptr) {
            zwp_locked_pointer_v1_destroy(locked_pointer_);
            locked_pointer_ = nullptr;
        }
        if (relative_pointer_ != nullptr) {
            zwp_relative_pointer_v1_destroy(relative_pointer_);
            relative_pointer_ = nullptr;
        }
        if (display_ != nullptr) {
            wl_display_flush(display_);
        }
    }

    bool captured() const override {
        return captured_;
    }

private:
    void initialize() {
        auto* platform = QGuiApplication::platformNativeInterface();
        if (platform == nullptr) {
            return;
        }
        display_ = static_cast<wl_display*>(
            platform->nativeResourceForIntegration("wl_display"));
        if (display_ == nullptr) {
            return;
        }
        registry_ = wl_display_get_registry(display_);
        if (registry_ == nullptr) {
            display_ = nullptr;
            return;
        }
        wl_registry_add_listener(registry_, &registry_listener_, this);
        if (wl_display_roundtrip(display_) < 0) {
            display_ = nullptr;
        }
    }

    static void registry_global(
        void* data,
        wl_registry* registry,
        std::uint32_t name,
        const char* interface,
        std::uint32_t version) {
        auto& self = *static_cast<WaylandMouseCapture*>(data);
        if (std::strcmp(
                interface, zwp_relative_pointer_manager_v1_interface.name)
            == 0) {
            self.relative_pointer_manager_ =
                static_cast<zwp_relative_pointer_manager_v1*>(
                    wl_registry_bind(
                        registry,
                        name,
                        &zwp_relative_pointer_manager_v1_interface,
                        std::min(version, 1U)));
            self.relative_pointer_manager_name_ = name;
        } else if (std::strcmp(
                       interface, zwp_pointer_constraints_v1_interface.name)
                   == 0) {
            self.pointer_constraints_ =
                static_cast<zwp_pointer_constraints_v1*>(wl_registry_bind(
                    registry,
                    name,
                    &zwp_pointer_constraints_v1_interface,
                    std::min(version, 1U)));
            self.pointer_constraints_name_ = name;
        }
    }

    static void registry_global_remove(
        void* data,
        wl_registry*,
        std::uint32_t name) {
        auto& self = *static_cast<WaylandMouseCapture*>(data);
        if (name == self.relative_pointer_manager_name_) {
            self.release();
            zwp_relative_pointer_manager_v1_destroy(
                self.relative_pointer_manager_);
            self.relative_pointer_manager_ = nullptr;
            self.relative_pointer_manager_name_ = 0;
        }
        if (name == self.pointer_constraints_name_) {
            self.release();
            zwp_pointer_constraints_v1_destroy(self.pointer_constraints_);
            self.pointer_constraints_ = nullptr;
            self.pointer_constraints_name_ = 0;
        }
    }

    static void relative_motion(
        void* data,
        zwp_relative_pointer_v1*,
        std::uint32_t,
        std::uint32_t,
        wl_fixed_t,
        wl_fixed_t,
        wl_fixed_t delta_x_unaccelerated,
        wl_fixed_t delta_y_unaccelerated) {
        auto& self = *static_cast<WaylandMouseCapture*>(data);
        if (self.captured_) {
            self.motion_handler_(
                wl_fixed_to_double(delta_x_unaccelerated),
                wl_fixed_to_double(delta_y_unaccelerated));
        }
    }

    static void pointer_locked(void* data, zwp_locked_pointer_v1*) {
        static_cast<WaylandMouseCapture*>(data)->lock_active_ = true;
    }

    static void pointer_unlocked(void* data, zwp_locked_pointer_v1*) {
        static_cast<WaylandMouseCapture*>(data)->lock_active_ = false;
    }

    inline static const wl_registry_listener registry_listener_{
        registry_global,
        registry_global_remove,
    };
    inline static const zwp_relative_pointer_v1_listener
        relative_pointer_listener_{
            relative_motion,
        };
    inline static const zwp_locked_pointer_v1_listener
        locked_pointer_listener_{
            pointer_locked,
            pointer_unlocked,
        };

    RelativeMotionHandler motion_handler_;
    wl_display* display_ = nullptr;
    wl_registry* registry_ = nullptr;
    zwp_relative_pointer_manager_v1* relative_pointer_manager_ = nullptr;
    zwp_pointer_constraints_v1* pointer_constraints_ = nullptr;
    zwp_relative_pointer_v1* relative_pointer_ = nullptr;
    zwp_locked_pointer_v1* locked_pointer_ = nullptr;
    std::uint32_t relative_pointer_manager_name_ = 0;
    std::uint32_t pointer_constraints_name_ = 0;
    bool captured_ = false;
    bool lock_active_ = false;
};

} // namespace

std::unique_ptr<HostMouseCaptureBackend> make_wayland_mouse_capture(
    RelativeMotionHandler motion_handler) {
    return std::make_unique<WaylandMouseCapture>(std::move(motion_handler));
}

} // namespace se_ui::frontend
