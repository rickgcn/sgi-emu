#pragma once

#include "rust/cxx.h"

#include <cstdint>
#include <memory>

namespace se_ui {

enum class VideoOutputStateDto : std::uint8_t;
struct VideoFrameHandle;
class DisplayWorkspace;
class SerialConsoleDock;

class MachineOutputSink final : public std::enable_shared_from_this<MachineOutputSink> {
public:
    MachineOutputSink(SerialConsoleDock* console, DisplayWorkspace* workspace);
    ~MachineOutputSink();

    void publish_serial(std::uint64_t generation, rust::Str key, rust::Slice<const std::uint8_t> bytes) const;
    void publish_video(std::uint64_t generation, rust::Str key, VideoOutputStateDto state, rust::Box<VideoFrameHandle> frame) const;

private:
    struct PendingOutput;

    void schedule_delivery(bool required) const;
    void drain() const;

    SerialConsoleDock* console_;
    DisplayWorkspace* workspace_;
    std::unique_ptr<PendingOutput> pending_;
};

} // namespace se_ui
