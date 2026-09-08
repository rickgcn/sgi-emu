#pragma once

#include "rust/cxx.h"

#include <QWidget>

#include <cstdint>
#include <memory>

class QPaintEvent;

namespace se_ui {

enum class VideoOutputStateDto : std::uint8_t;
struct VideoFrameHandle;

class DisplayWidget final : public QWidget {
public:
    explicit DisplayWidget(QWidget* parent = nullptr);
    ~DisplayWidget() override;

    void set_video_output(
        VideoOutputStateDto state,
        rust::Box<VideoFrameHandle> frame);

protected:
    void paintEvent(QPaintEvent* event) override;

private:
    struct State;

    std::unique_ptr<State> state_;
};

} // namespace se_ui
