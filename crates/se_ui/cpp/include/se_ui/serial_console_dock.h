#pragma once

#include "rust/cxx.h"

#include <QDockWidget>

#include <cstdint>
#include <functional>
#include <memory>
#include <mutex>
#include <vector>

namespace se_ui {

enum class SerialPortDto : std::uint8_t;
struct RuntimeStatusDto;
struct UiSession;
class Vt100Widget;

class SerialConsoleDock final : public QDockWidget {
public:
    using StatusHandler = std::function<void(const RuntimeStatusDto&)>;

    SerialConsoleDock(
        const UiSession& session,
        StatusHandler status_handler,
        QWidget* parent = nullptr);

    void append_serial(
        const std::vector<std::uint8_t>& serial_a,
        const std::vector<std::uint8_t>& serial_b);

private:
    void send_serial(SerialPortDto port, const std::vector<std::uint8_t>& bytes) const;

    const UiSession& session_;
    StatusHandler status_handler_;
    Vt100Widget* serial_a_;
    Vt100Widget* serial_b_;
};

class MachineOutputSink final : public std::enable_shared_from_this<MachineOutputSink> {
public:
    explicit MachineOutputSink(SerialConsoleDock* console);

    void publish_serial(
        rust::Slice<const std::uint8_t> serial_a,
        rust::Slice<const std::uint8_t> serial_b) const;

private:
    void drain() const;

    SerialConsoleDock* console_;
    mutable std::mutex mutex_;
    mutable std::vector<std::uint8_t> pending_serial_a_;
    mutable std::vector<std::uint8_t> pending_serial_b_;
    mutable bool delivery_scheduled_;
};

} // namespace se_ui
