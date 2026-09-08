#include "se_ui/machine_output_sink.h"

#include "se_ui/display_widget.h"
#include "se_ui/serial_console_dock.h"
#include "se_ui/src/bridge.rs.h"

#include <QMetaObject>

#include <mutex>
#include <optional>
#include <utility>
#include <vector>

namespace se_ui {

struct MachineOutputSink::PendingOutput {
    std::mutex mutex;
    std::vector<std::uint8_t> serial_a;
    std::vector<std::uint8_t> serial_b;
    std::optional<VideoOutputStateDto> video_state;
    std::optional<rust::Box<VideoFrameHandle>> video_frame;
    bool delivery_scheduled = false;
};

MachineOutputSink::MachineOutputSink(SerialConsoleDock* console, DisplayWidget* display)
    : console_(console)
    , display_(display)
    , pending_(std::make_unique<PendingOutput>()) {
}

MachineOutputSink::~MachineOutputSink() = default;

void MachineOutputSink::publish_serial(
    rust::Slice<const std::uint8_t> serial_a,
    rust::Slice<const std::uint8_t> serial_b) const {
    if (serial_a.empty() && serial_b.empty()) {
        return;
    }

    bool schedule = false;
    {
        const std::lock_guard lock(pending_->mutex);
        pending_->serial_a.insert(pending_->serial_a.end(), serial_a.begin(), serial_a.end());
        pending_->serial_b.insert(pending_->serial_b.end(), serial_b.begin(), serial_b.end());
        if (!pending_->delivery_scheduled) {
            pending_->delivery_scheduled = true;
            schedule = true;
        }
    }
    schedule_delivery(schedule);
}

void MachineOutputSink::publish_video(
    VideoOutputStateDto state,
    rust::Box<VideoFrameHandle> frame) const {
    bool schedule = false;
    {
        const std::lock_guard lock(pending_->mutex);
        pending_->video_state = state;
        pending_->video_frame.reset();
        pending_->video_frame.emplace(std::move(frame));
        if (!pending_->delivery_scheduled) {
            pending_->delivery_scheduled = true;
            schedule = true;
        }
    }
    schedule_delivery(schedule);
}

void MachineOutputSink::schedule_delivery(bool required) const {
    if (!required) {
        return;
    }

    const auto self = shared_from_this();
    if (!QMetaObject::invokeMethod(
            display_, [self] { self->drain(); }, Qt::QueuedConnection)) {
        const std::lock_guard lock(pending_->mutex);
        pending_->delivery_scheduled = false;
    }
}

void MachineOutputSink::drain() const {
    std::vector<std::uint8_t> serial_a;
    std::vector<std::uint8_t> serial_b;
    std::optional<VideoOutputStateDto> video_state;
    std::optional<rust::Box<VideoFrameHandle>> video_frame;
    {
        const std::lock_guard lock(pending_->mutex);
        serial_a.swap(pending_->serial_a);
        serial_b.swap(pending_->serial_b);
        video_state.swap(pending_->video_state);
        video_frame.swap(pending_->video_frame);
        pending_->delivery_scheduled = false;
    }

    console_->append_serial(serial_a, serial_b);
    if (video_state.has_value() && video_frame.has_value()) {
        display_->set_video_output(*video_state, std::move(*video_frame));
    }
}

} // namespace se_ui
