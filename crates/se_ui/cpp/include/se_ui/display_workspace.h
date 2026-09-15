#pragma once

#include "se_ui/endpoint_identity.h"

#include <QWidget>

#include <cstdint>
#include <utility>
#include <vector>

class QLabel;
class QStackedWidget;
class QTabWidget;

namespace se_ui {

struct EndpointCatalogDto;
struct UiSession;
struct VideoFrameHandle;
enum class VideoOutputStateDto : std::uint8_t;
class DisplayWidget;

class DisplayWorkspace final : public QWidget {
public:
    explicit DisplayWorkspace(const UiSession& session, QWidget* parent = nullptr);

    void rebuild(const EndpointCatalogDto& catalog);
    void set_input_enabled(bool enabled);
    void release_input();
    void publish_video(std::uint64_t generation, rust::Str key, VideoOutputStateDto state, rust::Box<VideoFrameHandle> frame);

private:
    const UiSession& session_;
    QStackedWidget* stack_;
    QLabel* empty_;
    QTabWidget* tabs_;
    std::vector<std::pair<EndpointIdentity, DisplayWidget*>> displays_;
    bool input_enabled_ = false;
};

} // namespace se_ui
