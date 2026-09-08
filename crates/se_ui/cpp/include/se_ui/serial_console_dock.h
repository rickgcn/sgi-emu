#pragma once

#include <QDockWidget>

#include <cstdint>
#include <functional>
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
    void set_input_enabled(bool enabled);

private:
    void send_serial(SerialPortDto port, const std::vector<std::uint8_t>& bytes) const;

    const UiSession& session_;
    StatusHandler status_handler_;
    Vt100Widget* serial_a_;
    Vt100Widget* serial_b_;
    bool input_enabled_;
};

} // namespace se_ui
