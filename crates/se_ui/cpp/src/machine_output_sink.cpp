#include "se_ui/machine_output_sink.h"

#include "se_ui/display_workspace.h"
#include "se_ui/endpoint_identity.h"
#include "se_ui/serial_console_dock.h"
#include "se_ui/src/bridge.rs.h"

#include <QMetaObject>

#include <algorithm>
#include <mutex>
#include <utility>
#include <vector>

namespace se_ui {

struct MachineOutputSink::PendingOutput {
    struct Serial {
        EndpointIdentity identity;
        std::vector<std::uint8_t> bytes;
    };
    struct Video {
        EndpointIdentity identity;
        VideoOutputStateDto state;
        rust::Box<VideoFrameHandle> frame;
    };

    std::mutex mutex;
    std::vector<Serial> serial;
    std::vector<Video> video;
    bool delivery_scheduled = false;
};

MachineOutputSink::MachineOutputSink(SerialConsoleDock* console, DisplayWorkspace* workspace)
    : console_(console)
    , workspace_(workspace)
    , pending_(std::make_unique<PendingOutput>()) {
}

MachineOutputSink::~MachineOutputSink() = default;

void MachineOutputSink::publish_serial(std::uint64_t generation, rust::Str key, rust::Slice<const std::uint8_t> bytes) const {
    if (bytes.empty()) {
        return;
    }
    const EndpointIdentity identity{generation, std::string(key.data(), key.size())};
    bool schedule = false;
    {
        const std::lock_guard lock(pending_->mutex);
        auto found = std::find_if(pending_->serial.begin(), pending_->serial.end(),
            [&](const auto& item) { return item.identity == identity; });
        if (found == pending_->serial.end()) {
            pending_->serial.push_back({identity, {bytes.begin(), bytes.end()}});
        } else {
            found->bytes.insert(found->bytes.end(), bytes.begin(), bytes.end());
        }
        if (!pending_->delivery_scheduled) {
            pending_->delivery_scheduled = true;
            schedule = true;
        }
    }
    schedule_delivery(schedule);
}

void MachineOutputSink::publish_video(std::uint64_t generation, rust::Str key, VideoOutputStateDto state, rust::Box<VideoFrameHandle> frame) const {
    const EndpointIdentity identity{generation, std::string(key.data(), key.size())};
    bool schedule = false;
    {
        const std::lock_guard lock(pending_->mutex);
        auto found = std::find_if(pending_->video.begin(), pending_->video.end(),
            [&](const auto& item) { return item.identity == identity; });
        if (found == pending_->video.end()) {
            pending_->video.push_back({identity, state, std::move(frame)});
        } else {
            found->state = state;
            found->frame = std::move(frame);
        }
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
    if (!QMetaObject::invokeMethod(workspace_, [self] { self->drain(); }, Qt::QueuedConnection)) {
        const std::lock_guard lock(pending_->mutex);
        pending_->delivery_scheduled = false;
    }
}

void MachineOutputSink::drain() const {
    std::vector<PendingOutput::Serial> serial;
    std::vector<PendingOutput::Video> video;
    {
        const std::lock_guard lock(pending_->mutex);
        serial.swap(pending_->serial);
        video.swap(pending_->video);
        pending_->delivery_scheduled = false;
    }
    for (const auto& item : serial) {
        console_->append_serial(item.identity.generation, rust::Str(item.identity.key.data(), item.identity.key.size()), item.bytes);
    }
    for (auto& item : video) {
        workspace_->publish_video(item.identity.generation, rust::Str(item.identity.key.data(), item.identity.key.size()), item.state, std::move(item.frame));
    }
}

} // namespace se_ui
