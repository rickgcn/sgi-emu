#pragma once

#include <QWindow>

#include <cstdint>
#include <functional>
#include <memory>
#include <string_view>

namespace se_ui::frontend {

using RelativeMotionHandler = std::function<void(double, double)>;

enum class HostMouseBackendKind {
    Unsupported,
    WindowsRawInput,
    XInput2,
    Wayland,
    MacOs,
};

constexpr bool platform_starts_with(
    std::string_view platform,
    std::string_view prefix) {
    return platform.size() >= prefix.size()
        && platform.substr(0, prefix.size()) == prefix;
}

constexpr HostMouseBackendKind select_host_mouse_backend(
    std::string_view platform) {
    if (platform_starts_with(platform, "wayland")) {
        return HostMouseBackendKind::Wayland;
    }
    if (platform == "xcb") {
        return HostMouseBackendKind::XInput2;
    }
    if (platform == "windows") {
        return HostMouseBackendKind::WindowsRawInput;
    }
    if (platform == "cocoa") {
        return HostMouseBackendKind::MacOs;
    }
    return HostMouseBackendKind::Unsupported;
}

static_assert(
    select_host_mouse_backend("wayland") == HostMouseBackendKind::Wayland);
static_assert(
    select_host_mouse_backend("wayland-egl") == HostMouseBackendKind::Wayland);
static_assert(
    select_host_mouse_backend("xcb") == HostMouseBackendKind::XInput2);

class HostMouseCaptureBackend {
public:
    virtual ~HostMouseCaptureBackend() = default;

    virtual bool capture(QWindow* target) = 0;
    virtual void release() = 0;
    virtual bool captured() const = 0;
};

std::unique_ptr<HostMouseCaptureBackend> make_windows_mouse_capture(
    RelativeMotionHandler motion_handler);
std::unique_ptr<HostMouseCaptureBackend> make_xinput2_mouse_capture(
    RelativeMotionHandler motion_handler);
std::unique_ptr<HostMouseCaptureBackend> make_wayland_mouse_capture(
    RelativeMotionHandler motion_handler);
std::unique_ptr<HostMouseCaptureBackend> make_macos_mouse_capture(
    RelativeMotionHandler motion_handler);

} // namespace se_ui::frontend
