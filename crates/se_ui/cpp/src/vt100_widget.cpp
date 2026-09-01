#include "se_ui/vt100_widget.h"

#include "se_ui/src/bridge.rs.h"

#include <QApplication>
#include <QClipboard>
#include <QContextMenuEvent>
#include <QFontDatabase>
#include <QFontMetrics>
#include <QKeyEvent>
#include <QMenu>
#include <QMouseEvent>
#include <QPainter>
#include <QPalette>
#include <QResizeEvent>
#include <QScrollBar>
#include <QSignalBlocker>

#include <algorithm>
#include <iterator>
#include <optional>
#include <utility>

namespace se_ui {
namespace {

constexpr std::uint8_t ATTRIBUTE_BOLD = 1;
constexpr std::uint8_t ATTRIBUTE_DIM = 1 << 1;
constexpr std::uint8_t ATTRIBUTE_ITALIC = 1 << 2;
constexpr std::uint8_t ATTRIBUTE_UNDERLINE = 1 << 3;
constexpr std::uint8_t ATTRIBUTE_INVERSE = 1 << 4;

struct SelectionPoint {
    std::uint32_t row;
    std::uint16_t column;
};

bool point_less(const SelectionPoint& left, const SelectionPoint& right) {
    return left.row < right.row || (left.row == right.row && left.column < right.column);
}

QColor indexed_color(std::uint32_t index) {
    static const QColor ANSI_COLORS[] = {
        QColor(0, 0, 0),
        QColor(170, 0, 0),
        QColor(0, 170, 0),
        QColor(170, 85, 0),
        QColor(0, 0, 170),
        QColor(170, 0, 170),
        QColor(0, 170, 170),
        QColor(170, 170, 170),
        QColor(85, 85, 85),
        QColor(255, 85, 85),
        QColor(85, 255, 85),
        QColor(255, 255, 85),
        QColor(85, 85, 255),
        QColor(255, 85, 255),
        QColor(85, 255, 255),
        QColor(255, 255, 255),
    };
    if (index < std::size(ANSI_COLORS)) {
        return ANSI_COLORS[index];
    }
    if (index < 232) {
        const auto cube = index - 16;
        const auto component = [](std::uint32_t value) {
            return value == 0 ? 0 : static_cast<int>(55 + value * 40);
        };
        return QColor(
            component(cube / 36), component((cube / 6) % 6), component(cube % 6));
    }
    const auto gray = 8 + static_cast<int>(std::min(index, 255U) - 232U) * 10;
    return QColor(gray, gray, gray);
}

QColor terminal_color(const TerminalColorDto& color, const QColor& default_color) {
    if (color.kind == 1) {
        return indexed_color(color.value);
    }
    if (color.kind == 2) {
        return QColor(
            static_cast<int>((color.value >> 16) & 0xff),
            static_cast<int>((color.value >> 8) & 0xff),
            static_cast<int>(color.value & 0xff));
    }
    return default_color;
}

std::vector<std::uint8_t> copy_bytes(const rust::Vec<std::uint8_t>& bytes) {
    return {bytes.begin(), bytes.end()};
}

} // namespace

class Vt100Widget::Implementation {
public:
    explicit Implementation(Vt100Widget& owner)
        : owner_(owner)
        , model_(new_terminal_model())
        , snapshot_(model_->terminal_snapshot(0))
        , font_(QFontDatabase::systemFont(QFontDatabase::FixedFont))
        , cell_width_(std::max(1, QFontMetrics(font_).horizontalAdvance(QLatin1Char('M'))))
        , cell_height_(std::max(1, QFontMetrics(font_).height()))
        , ascent_(QFontMetrics(font_).ascent())
        , selecting_(false)
        , has_selection_(false) {
        owner_.setFont(font_);
        owner_.setFocusPolicy(Qt::StrongFocus);
        owner_.viewport()->setCursor(Qt::IBeamCursor);
        owner_.horizontalScrollBar()->setSingleStep(cell_width_);
        owner_.verticalScrollBar()->setSingleStep(1);
        QObject::connect(owner_.verticalScrollBar(), &QScrollBar::valueChanged, &owner_, [this] {
            load_scroll_position();
        });
        update_scrollbars(true);
    }

    void set_input_handler(InputHandler handler) {
        input_handler_ = std::move(handler);
    }

    void feed(const std::vector<std::uint8_t>& bytes) {
        if (bytes.empty()) {
            return;
        }
        const bool follow = owner_.verticalScrollBar()->value()
            == owner_.verticalScrollBar()->maximum();
        snapshot_ = model_->terminal_feed(
            rust::Slice<const std::uint8_t>(bytes.data(), bytes.size()));
        update_scrollbars(follow);
        owner_.viewport()->update();
    }

    void clear_terminal() {
        snapshot_ = model_->terminal_clear();
        selecting_ = false;
        has_selection_ = false;
        update_scrollbars(true);
        owner_.viewport()->update();
    }

    void paint() {
        QPainter painter(owner_.viewport());
        painter.fillRect(owner_.viewport()->rect(), owner_.palette().color(QPalette::Base));
        painter.setFont(font_);

        const auto visible_rows = visible_row_count();
        const auto snapshot_top = snapshot_.scrollback_rows - snapshot_.scrollback_offset;
        const auto requested_top = static_cast<std::uint32_t>(owner_.verticalScrollBar()->value());
        const auto first_snapshot_row = requested_top > snapshot_top ? requested_top - snapshot_top : 0;
        const auto horizontal_offset = owner_.horizontalScrollBar()->value();
        const auto columns = static_cast<std::size_t>(snapshot_.columns);

        for (int display_row = 0; display_row < visible_rows; ++display_row) {
            const auto snapshot_row = first_snapshot_row + static_cast<std::uint32_t>(display_row);
            if (snapshot_row >= snapshot_.rows) {
                break;
            }
            for (std::uint16_t column = 0; column < snapshot_.columns; ++column) {
                const auto index = static_cast<std::size_t>(snapshot_row) * columns + column;
                if (index >= snapshot_.cells.size()) {
                    continue;
                }
                const auto& cell = snapshot_.cells[index];
                const QRect rectangle(
                    static_cast<int>(column) * cell_width_ - horizontal_offset,
                    display_row * cell_height_,
                    cell_width_,
                    cell_height_);
                if (!rectangle.intersects(owner_.viewport()->rect())) {
                    continue;
                }

                QColor foreground = terminal_color(
                    cell.foreground, owner_.palette().color(QPalette::Text));
                QColor background = terminal_color(
                    cell.background, owner_.palette().color(QPalette::Base));
                if ((cell.attributes & ATTRIBUTE_INVERSE) != 0) {
                    std::swap(foreground, background);
                }
                const SelectionPoint point {
                    requested_top + static_cast<std::uint32_t>(display_row), column};
                if (is_selected(point)) {
                    foreground = owner_.palette().color(QPalette::HighlightedText);
                    background = owner_.palette().color(QPalette::Highlight);
                }
                painter.fillRect(rectangle, background);

                QFont cell_font = font_;
                cell_font.setBold((cell.attributes & ATTRIBUTE_BOLD) != 0);
                cell_font.setItalic((cell.attributes & ATTRIBUTE_ITALIC) != 0);
                cell_font.setUnderline((cell.attributes & ATTRIBUTE_UNDERLINE) != 0);
                painter.setFont(cell_font);
                if ((cell.attributes & ATTRIBUTE_DIM) != 0) {
                    foreground.setAlphaF(0.55);
                }
                painter.setPen(foreground);
                painter.drawText(
                    rectangle.left(), rectangle.top() + ascent_,
                    QString::fromUtf8(cell.text.data(), static_cast<qsizetype>(cell.text.size())));
            }
        }

        if (snapshot_.cursor_visible && snapshot_.scrollback_offset == 0) {
            const auto cursor_absolute_row = snapshot_.scrollback_rows + snapshot_.cursor_row;
            if (cursor_absolute_row >= requested_top
                && cursor_absolute_row < requested_top + static_cast<std::uint32_t>(visible_rows)) {
                const QRect cursor(
                    static_cast<int>(snapshot_.cursor_column) * cell_width_ - horizontal_offset,
                    static_cast<int>(cursor_absolute_row - requested_top) * cell_height_,
                    cell_width_, cell_height_);
                painter.setCompositionMode(QPainter::RasterOp_SourceXorDestination);
                painter.fillRect(cursor, owner_.palette().color(QPalette::Text));
            }
        }
    }

    void resize() {
        update_scrollbars(owner_.verticalScrollBar()->value()
            == owner_.verticalScrollBar()->maximum());
    }

    void scroll() {
        owner_.viewport()->update();
    }

    void key_press(QKeyEvent& event) {
        const auto modifiers = event.modifiers();
        if (modifiers == (Qt::ControlModifier | Qt::ShiftModifier)
            && event.key() == Qt::Key_C) {
            copy_selection();
            return;
        }
        if (modifiers == (Qt::ControlModifier | Qt::ShiftModifier)
            && event.key() == Qt::Key_V) {
            paste_clipboard();
            return;
        }
        if (event.key() == Qt::Key_PageUp) {
            owner_.verticalScrollBar()->triggerAction(QAbstractSlider::SliderPageStepSub);
            return;
        }
        if (event.key() == Qt::Key_PageDown) {
            owner_.verticalScrollBar()->triggerAction(QAbstractSlider::SliderPageStepAdd);
            return;
        }

        std::vector<std::uint8_t> bytes;
        const bool keypad = modifiers.testFlag(Qt::KeypadModifier);
        if (modifiers.testFlag(Qt::ControlModifier)
            && !modifiers.testFlag(Qt::AltModifier)
            && !modifiers.testFlag(Qt::MetaModifier)
            && event.key() >= Qt::Key_At && event.key() <= Qt::Key_Underscore) {
            bytes = encode(TerminalKeyDto::Control, static_cast<std::uint8_t>(event.key()));
        } else if (keypad && keypad_value(event).has_value()) {
            bytes = encode(TerminalKeyDto::Keypad, *keypad_value(event));
        } else {
            const auto key = semantic_key(event.key());
            if (key.has_value()) {
                bytes = encode(*key, 0);
            } else if (!modifiers.testFlag(Qt::AltModifier)
                && !modifiers.testFlag(Qt::MetaModifier)
                && !modifiers.testFlag(Qt::ControlModifier)) {
                const auto utf8 = event.text().toUtf8();
                for (const auto value : utf8) {
                    if (static_cast<unsigned char>(value) <= 0x7f) {
                        const auto encoded = encode(
                            TerminalKeyDto::Text, static_cast<std::uint8_t>(value));
                        bytes.insert(bytes.end(), encoded.begin(), encoded.end());
                    }
                }
            }
        }
        send_input(bytes);
    }

    void mouse_press(QMouseEvent& event) {
        if (event.button() != Qt::LeftButton) {
            return;
        }
        owner_.setFocus();
        selection_anchor_ = point_at(event.position());
        selection_end_ = selection_anchor_;
        selecting_ = true;
        has_selection_ = false;
        owner_.viewport()->update();
    }

    void mouse_move(QMouseEvent& event) {
        if (!selecting_) {
            return;
        }
        if (event.position().y() < 0) {
            owner_.verticalScrollBar()->setValue(owner_.verticalScrollBar()->value() - 1);
        } else if (event.position().y() >= owner_.viewport()->height()) {
            owner_.verticalScrollBar()->setValue(owner_.verticalScrollBar()->value() + 1);
        }
        selection_end_ = point_at(event.position());
        has_selection_ = selection_anchor_.row != selection_end_.row
            || selection_anchor_.column != selection_end_.column;
        owner_.viewport()->update();
    }

    void mouse_release(QMouseEvent& event) {
        if (event.button() == Qt::LeftButton) {
            mouse_move(event);
            selecting_ = false;
        }
    }

    void context_menu(QContextMenuEvent& event) {
        QMenu menu(&owner_);
        auto* copy = menu.addAction(QStringLiteral("Copy"));
        copy->setEnabled(has_selection_);
        auto* paste = menu.addAction(QStringLiteral("Paste"));
        menu.addSeparator();
        auto* select_all = menu.addAction(QStringLiteral("Select All"));
        auto* clear = menu.addAction(QStringLiteral("Clear"));
        const auto* selected = menu.exec(event.globalPos());
        if (selected == copy) {
            copy_selection();
        } else if (selected == paste) {
            paste_clipboard();
        } else if (selected == select_all) {
            selection_anchor_ = {0, 0};
            selection_end_ = {
                snapshot_.scrollback_rows + snapshot_.rows - 1, snapshot_.columns};
            has_selection_ = true;
            owner_.viewport()->update();
        } else if (selected == clear) {
            clear_terminal();
        }
    }

private:
    int visible_row_count() const {
        return std::clamp(owner_.viewport()->height() / cell_height_, 1, static_cast<int>(snapshot_.rows));
    }

    void update_scrollbars(bool follow) {
        const auto content_width = static_cast<int>(snapshot_.columns) * cell_width_;
        auto* horizontal = owner_.horizontalScrollBar();
        horizontal->setPageStep(owner_.viewport()->width());
        horizontal->setRange(0, std::max(0, content_width - owner_.viewport()->width()));

        const auto visible_rows = visible_row_count();
        const auto total_rows = static_cast<int>(snapshot_.scrollback_rows + snapshot_.rows);
        const auto maximum = std::max(0, total_rows - visible_rows);
        auto* vertical = owner_.verticalScrollBar();
        const QSignalBlocker blocker(vertical);
        vertical->setPageStep(visible_rows);
        vertical->setRange(0, maximum);
        if (follow) {
            vertical->setValue(maximum);
            snapshot_ = model_->terminal_snapshot(0);
        } else if (snapshot_.scrollback_offset != 0) {
            vertical->setValue(static_cast<int>(
                snapshot_.scrollback_rows - snapshot_.scrollback_offset));
        } else {
            vertical->setValue(std::min(vertical->value(), maximum));
        }
    }

    void load_scroll_position() {
        const auto top = static_cast<std::uint32_t>(owner_.verticalScrollBar()->value());
        const auto offset = top < snapshot_.scrollback_rows
            ? snapshot_.scrollback_rows - top
            : 0;
        snapshot_ = model_->terminal_snapshot(offset);
        owner_.viewport()->update();
    }

    std::vector<std::uint8_t> encode(TerminalKeyDto key, std::uint8_t value) const {
        return copy_bytes(model_->terminal_encode_key(key, value));
    }

    void send_input(const std::vector<std::uint8_t>& bytes) const {
        if (!bytes.empty() && input_handler_) {
            input_handler_(bytes);
        }
    }

    void paste_clipboard() const {
        const auto text = QApplication::clipboard()->text();
        const auto utf8 = text.toUtf8();
        const auto bytes = normalize_terminal_paste(rust::Str(
            utf8.constData(), static_cast<std::size_t>(utf8.size())));
        send_input(copy_bytes(bytes));
    }

    void copy_selection() {
        if (!has_selection_) {
            return;
        }
        auto start = selection_anchor_;
        auto end = selection_end_;
        if (point_less(end, start)) {
            std::swap(start, end);
        }
        const auto text = model_->terminal_selection(
            start.row, start.column, end.row, end.column);
        QApplication::clipboard()->setText(
            QString::fromUtf8(text.data(), static_cast<qsizetype>(text.size())));
    }

    SelectionPoint point_at(const QPointF& position) const {
        const auto x = std::clamp(
            static_cast<int>(position.x()) + owner_.horizontalScrollBar()->value(),
            0,
            static_cast<int>(snapshot_.columns) * cell_width_);
        const auto row = std::clamp(
            static_cast<int>(position.y()) / cell_height_, 0, visible_row_count() - 1);
        return {
            static_cast<std::uint32_t>(owner_.verticalScrollBar()->value() + row),
            static_cast<std::uint16_t>(
                std::clamp(x / cell_width_, 0, static_cast<int>(snapshot_.columns))),
        };
    }

    bool is_selected(const SelectionPoint& point) const {
        if (!has_selection_) {
            return false;
        }
        auto start = selection_anchor_;
        auto end = selection_end_;
        if (point_less(end, start)) {
            std::swap(start, end);
        }
        return !point_less(point, start) && point_less(point, end);
    }

    static std::optional<TerminalKeyDto> semantic_key(int key) {
        switch (key) {
        case Qt::Key_Up:
            return TerminalKeyDto::Up;
        case Qt::Key_Down:
            return TerminalKeyDto::Down;
        case Qt::Key_Right:
            return TerminalKeyDto::Right;
        case Qt::Key_Left:
            return TerminalKeyDto::Left;
        case Qt::Key_F1:
            return TerminalKeyDto::Pf1;
        case Qt::Key_F2:
            return TerminalKeyDto::Pf2;
        case Qt::Key_F3:
            return TerminalKeyDto::Pf3;
        case Qt::Key_F4:
            return TerminalKeyDto::Pf4;
        case Qt::Key_Return:
        case Qt::Key_Enter:
            return TerminalKeyDto::Enter;
        case Qt::Key_Backspace:
            return TerminalKeyDto::Backspace;
        case Qt::Key_Tab:
            return TerminalKeyDto::Tab;
        case Qt::Key_Escape:
            return TerminalKeyDto::Escape;
        case Qt::Key_Delete:
            return TerminalKeyDto::Delete;
        default:
            return std::nullopt;
        }
    }

    static std::optional<std::uint8_t> keypad_value(const QKeyEvent& event) {
        switch (event.key()) {
        case Qt::Key_0:
        case Qt::Key_1:
        case Qt::Key_2:
        case Qt::Key_3:
        case Qt::Key_4:
        case Qt::Key_5:
        case Qt::Key_6:
        case Qt::Key_7:
        case Qt::Key_8:
        case Qt::Key_9:
            return static_cast<std::uint8_t>(event.key());
        case Qt::Key_Period:
            return static_cast<std::uint8_t>('.');
        case Qt::Key_Plus:
            return static_cast<std::uint8_t>('+');
        case Qt::Key_Minus:
            return static_cast<std::uint8_t>('-');
        case Qt::Key_Asterisk:
            return static_cast<std::uint8_t>('*');
        case Qt::Key_Slash:
            return static_cast<std::uint8_t>('/');
        case Qt::Key_Enter:
            return static_cast<std::uint8_t>('\r');
        default:
            return std::nullopt;
        }
    }

    Vt100Widget& owner_;
    rust::Box<TerminalModel> model_;
    TerminalSnapshotDto snapshot_;
    QFont font_;
    int cell_width_;
    int cell_height_;
    int ascent_;
    InputHandler input_handler_;
    bool selecting_;
    bool has_selection_;
    SelectionPoint selection_anchor_ {};
    SelectionPoint selection_end_ {};
};

Vt100Widget::Vt100Widget(QWidget* parent)
    : QAbstractScrollArea(parent)
    , implementation_(std::make_unique<Implementation>(*this)) {
}

Vt100Widget::~Vt100Widget() = default;

void Vt100Widget::set_input_handler(InputHandler handler) {
    implementation_->set_input_handler(std::move(handler));
}

void Vt100Widget::feed(const std::vector<std::uint8_t>& bytes) {
    implementation_->feed(bytes);
}

void Vt100Widget::clear_terminal() {
    implementation_->clear_terminal();
}

void Vt100Widget::contextMenuEvent(QContextMenuEvent* event) {
    implementation_->context_menu(*event);
}

void Vt100Widget::keyPressEvent(QKeyEvent* event) {
    implementation_->key_press(*event);
}

void Vt100Widget::mouseMoveEvent(QMouseEvent* event) {
    implementation_->mouse_move(*event);
}

void Vt100Widget::mousePressEvent(QMouseEvent* event) {
    implementation_->mouse_press(*event);
}

void Vt100Widget::mouseReleaseEvent(QMouseEvent* event) {
    implementation_->mouse_release(*event);
}

void Vt100Widget::paintEvent(QPaintEvent*) {
    implementation_->paint();
}

void Vt100Widget::resizeEvent(QResizeEvent* event) {
    QAbstractScrollArea::resizeEvent(event);
    implementation_->resize();
}

void Vt100Widget::scrollContentsBy(int, int) {
    implementation_->scroll();
}

} // namespace se_ui
