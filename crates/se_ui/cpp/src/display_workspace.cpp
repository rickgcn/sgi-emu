#include "se_ui/display_workspace.h"

#include "se_ui/display_widget.h"
#include "se_ui/src/bridge.rs.h"

#include <QLabel>
#include <QStackedWidget>
#include <QTabWidget>
#include <QVBoxLayout>

#include <cstddef>
#include <optional>

namespace se_ui {

DisplayWorkspace::DisplayWorkspace(const UiSession& session, QWidget* parent)
    : QWidget(parent)
    , session_(session)
    , stack_(new QStackedWidget(this))
    , empty_(new QLabel(QStringLiteral("No video outputs"), stack_))
    , tabs_(new QTabWidget(stack_)) {
    setObjectName(QStringLiteral("DisplayWorkspace"));
    empty_->setAlignment(Qt::AlignCenter);
    stack_->addWidget(empty_);
    stack_->addWidget(tabs_);
    stack_->setCurrentWidget(empty_);
    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(stack_);
}

void DisplayWorkspace::rebuild(const EndpointCatalogDto& catalog) {
    for (const auto& [_, display] : displays_) {
        display->release_input();
    }
    while (tabs_->count() != 0) {
        auto* widget = tabs_->widget(0);
        tabs_->removeTab(0);
        delete widget;
    }
    displays_.clear();

    std::optional<EndpointIdentity> keyboard;
    std::optional<EndpointIdentity> pointer;
    std::size_t keyboards = 0;
    std::size_t pointers = 0;
    for (const auto& descriptor : catalog.endpoints) {
        if (descriptor.kind == EndpointKindDto::Keyboard) {
            keyboard = endpoint_identity(descriptor.handle);
            ++keyboards;
        } else if (descriptor.kind == EndpointKindDto::Pointer) {
            pointer = endpoint_identity(descriptor.handle);
            ++pointers;
        }
    }
    if (keyboards != 1) {
        keyboard.reset();
    }
    if (pointers != 1) {
        pointer.reset();
    }

    for (const auto& descriptor : catalog.endpoints) {
        if (descriptor.kind != EndpointKindDto::Video) {
            continue;
        }
        auto* display = new DisplayWidget(session_, tabs_);
        display->set_input_endpoints(keyboard, pointer);
        display->set_input_enabled(input_enabled_);
        tabs_->addTab(display, QString::fromUtf8(descriptor.label.data(), static_cast<qsizetype>(descriptor.label.size())));
        displays_.emplace_back(endpoint_identity(descriptor.handle), display);
    }
    stack_->setCurrentWidget(displays_.empty() ? static_cast<QWidget*>(empty_) : static_cast<QWidget*>(tabs_));
}

void DisplayWorkspace::set_input_enabled(bool enabled) {
    input_enabled_ = enabled;
    for (const auto& [_, display] : displays_) {
        display->set_input_enabled(enabled);
    }
}

void DisplayWorkspace::release_input() {
    for (const auto& [_, display] : displays_) {
        display->release_input();
    }
}

void DisplayWorkspace::publish_video(std::uint64_t generation, rust::Str key, VideoOutputStateDto state, rust::Box<VideoFrameHandle> frame) {
    const EndpointIdentity identity{generation, std::string(key.data(), key.size())};
    for (const auto& [candidate, display] : displays_) {
        if (candidate == identity) {
            display->set_video_output(state, std::move(frame));
            return;
        }
    }
}

} // namespace se_ui
