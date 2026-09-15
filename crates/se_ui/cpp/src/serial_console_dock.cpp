#include "se_ui/serial_console_dock.h"

#include "se_ui/src/bridge.rs.h"
#include "se_ui/vt100_widget.h"

#include <QLabel>
#include <QStackedWidget>
#include <QTabWidget>

#include <cstddef>
#include <utility>

namespace se_ui {

SerialConsoleDock::SerialConsoleDock(const UiSession& session, StatusHandler status_handler, QWidget* parent)
    : QDockWidget(QStringLiteral("Serial Console"), parent)
    , session_(session)
    , status_handler_(std::move(status_handler))
    , stack_(new QStackedWidget(this))
    , empty_(new QLabel(QStringLiteral("No serial terminals"), stack_))
    , tabs_(new QTabWidget(stack_))
    , terminals_()
    , input_enabled_(true) {
    setObjectName(QStringLiteral("SerialConsoleDock"));
    empty_->setAlignment(Qt::AlignCenter);
    stack_->addWidget(empty_);
    stack_->addWidget(tabs_);
    stack_->setCurrentWidget(empty_);
    setWidget(stack_);
}

void SerialConsoleDock::rebuild(const EndpointCatalogDto& catalog) {
    while (tabs_->count() != 0) {
        auto* widget = tabs_->widget(0);
        tabs_->removeTab(0);
        delete widget;
    }
    terminals_.clear();
    for (const auto& descriptor : catalog.endpoints) {
        if (descriptor.kind != EndpointKindDto::Serial || !descriptor.serial_console_attached) {
            continue;
        }
        const auto identity = endpoint_identity(descriptor.handle);
        auto* terminal = new Vt100Widget(tabs_);
        terminal->set_input_handler([this, identity](std::uint8_t value) { send_serial(identity, value); });
        tabs_->addTab(terminal, QString::fromUtf8(descriptor.label.data(), static_cast<qsizetype>(descriptor.label.size())));
        terminals_.emplace_back(identity, terminal);
    }
    stack_->setCurrentWidget(terminals_.empty() ? static_cast<QWidget*>(empty_) : static_cast<QWidget*>(tabs_));
}

void SerialConsoleDock::set_input_enabled(bool enabled) {
    input_enabled_ = enabled;
    if (!enabled) {
        for (const auto& terminal : terminals_) {
            terminal.second->discard_pending_input();
        }
    }
}

void SerialConsoleDock::append_serial(std::uint64_t generation, rust::Str key, const std::vector<std::uint8_t>& bytes) {
    const EndpointIdentity identity{generation, std::string(key.data(), key.size())};
    for (const auto& [candidate, terminal] : terminals_) {
        if (candidate == identity) {
            terminal->feed(bytes);
            return;
        }
    }
}

void SerialConsoleDock::send_serial(const EndpointIdentity& identity, std::uint8_t value) const {
    if (!input_enabled_) {
        return;
    }
    const auto handle = endpoint_handle_dto(identity);
    const auto status = session_.send_serial(handle, value);
    if (status_handler_) {
        status_handler_(status);
    }
}

} // namespace se_ui
