#include "se_ui/debugger/disassembly_dock.h"

#include "se_ui/src/bridge.rs.h"

#include <QAction>
#include <QCheckBox>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QKeySequence>
#include <QLineEdit>
#include <QMouseEvent>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QResizeEvent>
#include <QTextBlock>
#include <QTextCursor>
#include <QTextDocument>
#include <QVBoxLayout>
#include <QWidget>

#include <functional>
#include <limits>
#include <optional>
#include <utility>

namespace se_ui {
namespace {

constexpr std::uint32_t ROW_COUNT = 64;

class DisassemblyView final : public QPlainTextEdit {
public:
    DisassemblyView(std::function<void()> toggle_breakpoint, QWidget* parent)
        : QPlainTextEdit(parent)
        , toggle_breakpoint_(std::move(toggle_breakpoint)) {
    }

    void follow_block(int block_number, std::uint32_t pc) {
        const auto block = document()->findBlockByNumber(block_number);
        if (!block.isValid()) {
            return;
        }

        const bool center = !follow_target_.has_value()
            || (pc != follow_target_->pc && pc != follow_target_->pc + 4U);
        follow_target_ = FollowTarget {block_number, pc};

        setTextCursor(QTextCursor(block));
        if (center) {
            centerCursor();
        } else {
            ensureCursorVisible();
        }
    }

    void stop_following() {
        follow_target_.reset();
    }

protected:
    void mouseDoubleClickEvent(QMouseEvent* event) override {
        QPlainTextEdit::mouseDoubleClickEvent(event);
        toggle_breakpoint_();
    }

    void resizeEvent(QResizeEvent* event) override {
        QPlainTextEdit::resizeEvent(event);
        if (!follow_target_.has_value()) {
            return;
        }

        const auto block = document()->findBlockByNumber(follow_target_->block_number);
        if (block.isValid()) {
            setTextCursor(QTextCursor(block));
            ensureCursorVisible();
        }
    }

private:
    struct FollowTarget {
        int block_number;
        std::uint32_t pc;
    };

    std::function<void()> toggle_breakpoint_;
    std::optional<FollowTarget> follow_target_;
};

QString from_rust_string(const rust::String& value) {
    return QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size()));
}

} // namespace

DisassemblyDock::DisassemblyDock(const UiSession& session, QWidget* parent)
    : QDockWidget(QStringLiteral("Disassembly"), parent)
    , session_(session)
    , address_edit_(new QLineEdit(QStringLiteral("0xbfc00000"), this))
    , follow_pc_(new QCheckBox(QStringLiteral("Follow PC"), this))
    , text_view_(new DisassemblyView([this] { toggle_selected_breakpoint(); }, this))
    , start_(0xbfc0'0000)
    , revision_(std::numeric_limits<std::uint64_t>::max()) {
    setObjectName(QStringLiteral("DisassemblyDock"));
    follow_pc_->setChecked(true);
    text_view_->setReadOnly(true);
    text_view_->setLineWrapMode(QPlainTextEdit::NoWrap);
    text_view_->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));

    auto* go_button = new QPushButton(QStringLiteral("Go"), this);
    auto* controls = new QHBoxLayout;
    controls->addWidget(address_edit_);
    controls->addWidget(go_button);
    controls->addWidget(follow_pc_);

    auto* container = new QWidget(this);
    auto* layout = new QVBoxLayout(container);
    layout->setContentsMargins(6, 6, 6, 6);
    layout->addLayout(controls);
    layout->addWidget(text_view_);
    setWidget(container);

    connect(go_button, &QPushButton::clicked, this, &DisassemblyDock::apply_address);
    connect(address_edit_, &QLineEdit::returnPressed, this, &DisassemblyDock::apply_address);
    connect(follow_pc_, &QCheckBox::toggled, this, [this] {
        if (!follow_pc_->isChecked()) {
            static_cast<DisassemblyView*>(text_view_)->stop_following();
        }
        revision_ = std::numeric_limits<std::uint64_t>::max();
        refresh();
    });

    auto* toggle_breakpoint = new QAction(QStringLiteral("Toggle breakpoint"), text_view_);
    toggle_breakpoint->setShortcut(QKeySequence(QStringLiteral("F9")));
    toggle_breakpoint->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    connect(
        toggle_breakpoint,
        &QAction::triggered,
        this,
        &DisassemblyDock::toggle_selected_breakpoint);
    text_view_->addAction(toggle_breakpoint);
}

void DisassemblyDock::refresh() {
    if (follow_pc_->isChecked()) {
        const auto registers = session_.registers();
        if (registers.success) {
            const auto centered = registers.pc - 8U * 4U;
            if (registers.pc < start_ || registers.pc >= start_ + ROW_COUNT * 4U) {
                start_ = centered & ~std::uint32_t {3};
                address_edit_->setText(QStringLiteral("0x%1").arg(start_, 8, 16, QLatin1Char('0')));
                revision_ = std::numeric_limits<std::uint64_t>::max();
            }
        }
    }

    const auto data = session_.disassembly(start_, ROW_COUNT);
    if (!data.success) {
        clear();
        return;
    }
    if (data.revision == revision_) {
        return;
    }
    revision_ = data.revision;
    line_addresses_.clear();
    line_addresses_.reserve(data.lines.size());

    QString text;
    int current_block = -1;
    for (const auto& line : data.lines) {
        if (line.current) {
            current_block = static_cast<int>(line_addresses_.size());
        }
        line_addresses_.push_back(line.address);
        const auto marker = line.current ? QStringLiteral("=>")
            : line.breakpoint             ? QStringLiteral(" *")
                                          : QStringLiteral("  ");
        const auto word = line.readable
            ? QStringLiteral("%1").arg(line.word, 8, 16, QLatin1Char('0'))
            : QStringLiteral("????????");
        text += QStringLiteral("%1  %2  %3  %4\n")
                    .arg(marker)
                    .arg(line.address, 8, 16, QLatin1Char('0'))
                    .arg(word)
                    .arg(from_rust_string(line.text));
    }
    text_view_->setPlainText(text);
    if (follow_pc_->isChecked() && current_block >= 0) {
        const auto current_pc = line_addresses_[static_cast<std::size_t>(current_block)];
        static_cast<DisassemblyView*>(text_view_)->follow_block(current_block, current_pc);
    }
}

void DisassemblyDock::clear() {
    revision_ = std::numeric_limits<std::uint64_t>::max();
    line_addresses_.clear();
    static_cast<DisassemblyView*>(text_view_)->stop_following();
    text_view_->setPlainText(QStringLiteral("No machine configured."));
}

void DisassemblyDock::apply_address() {
    bool valid = false;
    auto text = address_edit_->text().trimmed();
    if (text.startsWith(QStringLiteral("0x"), Qt::CaseInsensitive)) {
        text.remove(0, 2);
    }
    const auto address = text.toUInt(&valid, 16);
    if (!valid) {
        return;
    }
    start_ = address & ~std::uint32_t {3};
    follow_pc_->setChecked(false);
    revision_ = std::numeric_limits<std::uint64_t>::max();
    refresh();
}

void DisassemblyDock::toggle_selected_breakpoint() {
    const auto line = text_view_->textCursor().blockNumber();
    if (line < 0 || static_cast<std::size_t>(line) >= line_addresses_.size()) {
        return;
    }
    const auto status = session_.toggle_breakpoint(line_addresses_[static_cast<std::size_t>(line)]);
    if (status.success) {
        revision_ = std::numeric_limits<std::uint64_t>::max();
        refresh();
    }
}

} // namespace se_ui
