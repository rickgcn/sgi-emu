#pragma once

#include "se_ui/endpoint_identity.h"

#include <QDockWidget>

#include <cstdint>
#include <functional>
#include <utility>
#include <vector>

class QLabel;
class QStackedWidget;
class QTabWidget;

namespace se_ui {

struct EndpointCatalogDto;
struct RuntimeStatusDto;
struct UiSession;
class Vt100Widget;

class SerialConsoleDock final : public QDockWidget {
public:
    using StatusHandler = std::function<void(const RuntimeStatusDto&)>;

    SerialConsoleDock(const UiSession& session, StatusHandler status_handler, QWidget* parent = nullptr);

    void rebuild(const EndpointCatalogDto& catalog);
    void append_serial(std::uint64_t generation, rust::Str key, const std::vector<std::uint8_t>& bytes);
    void set_input_enabled(bool enabled);

private:
    void send_serial(const EndpointIdentity& identity, std::uint8_t value) const;

    const UiSession& session_;
    StatusHandler status_handler_;
    QStackedWidget* stack_;
    QLabel* empty_;
    QTabWidget* tabs_;
    std::vector<std::pair<EndpointIdentity, Vt100Widget*>> terminals_;
    bool input_enabled_;
};

} // namespace se_ui
