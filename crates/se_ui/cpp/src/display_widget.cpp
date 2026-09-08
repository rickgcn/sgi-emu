#include "se_ui/display_widget.h"

#include "se_ui/src/bridge.rs.h"

#include <QImage>
#include <QPainter>
#include <QPaintEvent>

#include <optional>
#include <utility>

namespace se_ui {

struct DisplayWidget::State {
    VideoOutputStateDto output = VideoOutputStateDto::NoGraphicsBoard;
    std::optional<rust::Box<VideoFrameHandle>> frame;
    QImage image;
};

DisplayWidget::DisplayWidget(QWidget* parent)
    : QWidget(parent)
    , state_(std::make_unique<State>()) {
    setObjectName(QStringLiteral("DisplayWidget"));
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
    setAttribute(Qt::WA_OpaquePaintEvent);
}

DisplayWidget::~DisplayWidget() = default;

void DisplayWidget::set_video_output(
    VideoOutputStateDto output,
    rust::Box<VideoFrameHandle> frame) {
    state_->image = QImage();
    state_->frame.reset();
    state_->output = output;

    if (output == VideoOutputStateDto::Frame) {
        state_->frame.emplace(std::move(frame));
        const auto& retained = **state_->frame;
        const auto pixels = retained.pixels();
        const auto width = retained.width();
        const auto height = retained.height();
        state_->image = QImage(
            pixels.data(),
            static_cast<int>(width),
            static_cast<int>(height),
            static_cast<qsizetype>(width) * 4,
            QImage::Format_RGBA8888);
        if (state_->image.isNull()) {
            state_->frame.reset();
            state_->output = VideoOutputStateDto::Blank;
        }
    }

    update();
}

void DisplayWidget::paintEvent(QPaintEvent* event) {
    event->accept();
    QPainter painter(this);
    painter.fillRect(rect(), Qt::black);

    if (state_->output == VideoOutputStateDto::Frame && !state_->image.isNull()) {
        const auto scaled = state_->image.size().scaled(size(), Qt::KeepAspectRatio);
        QRect destination(QPoint(0, 0), scaled);
        destination.moveCenter(rect().center());
        painter.setRenderHint(QPainter::SmoothPixmapTransform, true);
        painter.drawImage(destination, state_->image);
        return;
    }

    QString message;
    if (state_->output == VideoOutputStateDto::NoGraphicsBoard) {
        message = QStringLiteral("No graphics board");
    } else if (state_->output == VideoOutputStateDto::NoSignal) {
        message = QStringLiteral("No signal");
    }
    if (!message.isEmpty()) {
        painter.setPen(Qt::white);
        painter.drawText(rect(), Qt::AlignCenter, message);
    }
}

} // namespace se_ui
