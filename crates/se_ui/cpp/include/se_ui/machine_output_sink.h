#pragma once

#include "rust/cxx.h"

#include <cstdint>
#include <memory>

namespace se_ui {

enum class VideoOutputStateDto : std::uint8_t;
struct VideoFrameHandle;
class DisplayWidget;
class SerialConsoleDock;

class MachineOutputSink final : public std::enable_shared_from_this<MachineOutputSink> {
public:
    MachineOutputSink(SerialConsoleDock* console, DisplayWidget* display);
    ~MachineOutputSink();

    void publish_serial(
        rust::Slice<const std::uint8_t> serial_a,
        rust::Slice<const std::uint8_t> serial_b) const;
    void publish_video(
        VideoOutputStateDto state,
        rust::Box<VideoFrameHandle> frame) const;

private:
    struct PendingOutput;

    void schedule_delivery(bool required) const;
    void drain() const;

    SerialConsoleDock* console_;
    DisplayWidget* display_;
    std::unique_ptr<PendingOutput> pending_;
};

} // namespace se_ui
