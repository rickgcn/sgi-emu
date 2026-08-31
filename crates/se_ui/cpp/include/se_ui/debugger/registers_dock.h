#pragma once

#include <QDockWidget>

#include <cstdint>

namespace se_ui {

struct RegistersDockData;
struct UiSession;

class RegistersDock final : public QDockWidget {
public:
    RegistersDock(const UiSession& session, QWidget* parent = nullptr);
    ~RegistersDock() override;

    void refresh();
    void clear();

private:
    const UiSession& session_;
    RegistersDockData* data_;
    std::uint64_t revision_;
};

} // namespace se_ui
