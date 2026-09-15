#pragma once

#include <functional>
#include <memory>

class QWindow;

namespace se_ui::frontend {

class HostMouseCapture final {
public:
    using RelativeMotionHandler = std::function<void(double, double)>;

    explicit HostMouseCapture(RelativeMotionHandler motion_handler);
    ~HostMouseCapture();

    HostMouseCapture(const HostMouseCapture&) = delete;
    HostMouseCapture& operator=(const HostMouseCapture&) = delete;

    bool capture(QWindow* target);
    void release();
    bool captured() const;

private:
    struct Implementation;
    std::unique_ptr<Implementation> implementation_;
};

} // namespace se_ui::frontend
