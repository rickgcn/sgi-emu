#pragma once

#include <QDockWidget>

#include <cstddef>
#include <cstdint>
#include <functional>

class QString;

namespace se_ui {

struct RegistersDockData;
struct UiSession;

class RegistersDock final : public QDockWidget {
public:
    RegistersDock(
        const UiSession& session,
        std::function<void(const QString&)> report_status,
        QWidget* parent = nullptr);
    ~RegistersDock() override;

    void refresh();
    void clear();

private:
    void copy_page(std::size_t page);

    const UiSession& session_;
    std::function<void(const QString&)> report_status_;
    RegistersDockData* data_;
    std::uint64_t revision_;
};

} // namespace se_ui
