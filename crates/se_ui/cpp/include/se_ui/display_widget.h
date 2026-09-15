#pragma once

#include "rust/cxx.h"
#include "se_ui/endpoint_identity.h"

#include <QWidget>

#include <cstdint>
#include <memory>
#include <optional>

class QPaintEvent;
class QEvent;
class QFocusEvent;
class QHideEvent;
class QKeyEvent;
class QMouseEvent;

namespace se_ui {

enum class VideoOutputStateDto : std::uint8_t;
enum class PointerButtonDto : std::uint8_t;
struct KeyboardKeyDto;
struct VideoFrameHandle;
struct UiSession;

class DisplayWidget final : public QWidget {
public:
    explicit DisplayWidget(const UiSession& session, QWidget* parent = nullptr);
    ~DisplayWidget() override;

    void set_video_output(
        VideoOutputStateDto state,
        rust::Box<VideoFrameHandle> frame);
    void set_input_enabled(bool enabled);
    void set_input_endpoints(std::optional<EndpointIdentity> keyboard, std::optional<EndpointIdentity> pointer);
    void release_input();

protected:
    bool event(QEvent* event) override;
    void focusOutEvent(QFocusEvent* event) override;
    void hideEvent(QHideEvent* event) override;
    void keyPressEvent(QKeyEvent* event) override;
    void keyReleaseEvent(QKeyEvent* event) override;
    void mouseMoveEvent(QMouseEvent* event) override;
    void mousePressEvent(QMouseEvent* event) override;
    void mouseReleaseEvent(QMouseEvent* event) override;
    void paintEvent(QPaintEvent* event) override;

private:
    struct State;

    void begin_pointer_capture();
    void end_pointer_capture();
    void abort_input();
    void release_guest_inputs();
    void handle_key(QKeyEvent* event, bool pressed);
    void handle_host_motion(double delta_x, double delta_y);
    void schedule_motion_delivery();
    bool drain_motion();
    bool send_keyboard(const KeyboardKeyDto& key, bool pressed) const;
    bool send_pointer_motion(std::int32_t delta_x, std::int32_t delta_y) const;
    bool send_pointer_button(PointerButtonDto button, bool pressed) const;

    std::unique_ptr<State> state_;
};

} // namespace se_ui
