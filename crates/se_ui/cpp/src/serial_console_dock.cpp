#include "se_ui/serial_console_dock.h"

#include "se_ui/src/bridge.rs.h"
#include "se_ui/vt100_widget.h"

#include <QString>
#include <QTabWidget>

#include <cstddef>
#include <utility>

namespace se_ui {

SerialConsoleDock::SerialConsoleDock(
    const UiSession& session,
    StatusHandler status_handler,
    QWidget* parent)
    : QDockWidget(QStringLiteral("Serial Console"), parent)
    , session_(session)
    , status_handler_(std::move(status_handler))
    , serial_a_(new Vt100Widget(this))
    , serial_b_(new Vt100Widget(this))
    , input_enabled_(true) {
    setObjectName(QStringLiteral("SerialConsoleDock"));
    serial_a_->set_input_handler(
        [this](const auto& bytes) { send_serial(SerialPortDto::A, bytes); });
    serial_b_->set_input_handler(
        [this](const auto& bytes) { send_serial(SerialPortDto::B, bytes); });

    auto* tabs = new QTabWidget(this);
    tabs->addTab(serial_a_, QStringLiteral("Serial A"));
    tabs->addTab(serial_b_, QStringLiteral("Serial B"));
    setWidget(tabs);
}

void SerialConsoleDock::set_input_enabled(bool enabled) {
    input_enabled_ = enabled;
}

void SerialConsoleDock::append_serial(
    const std::vector<std::uint8_t>& serial_a,
    const std::vector<std::uint8_t>& serial_b) {
    serial_a_->feed(serial_a);
    serial_b_->feed(serial_b);
}

void SerialConsoleDock::send_serial(
    SerialPortDto port,
    const std::vector<std::uint8_t>& bytes) const {
    if (bytes.empty() || !input_enabled_) {
        return;
    }
    const auto status = session_.send_serial(
        port, rust::Slice<const std::uint8_t>(bytes.data(), bytes.size()));
    if (status_handler_) {
        status_handler_(status);
    }
}

} // namespace se_ui
