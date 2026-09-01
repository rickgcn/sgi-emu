#pragma once

#include <QAbstractScrollArea>

#include <cstdint>
#include <functional>
#include <memory>
#include <vector>

class QContextMenuEvent;
class QKeyEvent;
class QMouseEvent;
class QPaintEvent;
class QResizeEvent;

namespace se_ui {

class Vt100Widget final : public QAbstractScrollArea {
public:
    using InputHandler = std::function<void(const std::vector<std::uint8_t>&)>;

    explicit Vt100Widget(QWidget* parent = nullptr);
    ~Vt100Widget() override;

    void set_input_handler(InputHandler handler);
    void feed(const std::vector<std::uint8_t>& bytes);
    void clear_terminal();

protected:
    void contextMenuEvent(QContextMenuEvent* event) override;
    void keyPressEvent(QKeyEvent* event) override;
    void mouseMoveEvent(QMouseEvent* event) override;
    void mousePressEvent(QMouseEvent* event) override;
    void mouseReleaseEvent(QMouseEvent* event) override;
    void paintEvent(QPaintEvent* event) override;
    void resizeEvent(QResizeEvent* event) override;
    void scrollContentsBy(int dx, int dy) override;

private:
    class Implementation;
    std::unique_ptr<Implementation> implementation_;
};

} // namespace se_ui
