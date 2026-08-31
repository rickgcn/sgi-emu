#include "se_ui/debugger/memory_dock.h"

#include "se_ui/src/bridge.rs.h"

#include <QComboBox>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QLineEdit>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>
#include <QWidget>

#include <limits>

namespace se_ui {
namespace {

constexpr std::uint32_t REQUEST_BYTES = 256;
constexpr std::size_t BYTES_PER_ROW = 16;

} // namespace

MemoryDock::MemoryDock(const UiSession& session, QWidget* parent)
    : QDockWidget(QStringLiteral("Memory"), parent)
    , session_(session)
    , address_space_(new QComboBox(this))
    , address_edit_(new QLineEdit(QStringLiteral("0x1fc00000"), this))
    , text_view_(new QPlainTextEdit(this))
    , start_(0x1fc0'0000)
    , revision_(std::numeric_limits<std::uint64_t>::max()) {
    setObjectName(QStringLiteral("MemoryDock"));
    address_space_->addItem(QStringLiteral("Physical"));
    address_space_->addItem(QStringLiteral("Virtual"));
    text_view_->setReadOnly(true);
    text_view_->setLineWrapMode(QPlainTextEdit::NoWrap);
    text_view_->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));

    auto* go_button = new QPushButton(QStringLiteral("Go"), this);
    auto* controls = new QHBoxLayout;
    controls->addWidget(address_space_);
    controls->addWidget(address_edit_);
    controls->addWidget(go_button);

    auto* container = new QWidget(this);
    auto* layout = new QVBoxLayout(container);
    layout->setContentsMargins(6, 6, 6, 6);
    layout->addLayout(controls);
    layout->addWidget(text_view_);
    setWidget(container);

    connect(go_button, &QPushButton::clicked, this, &MemoryDock::apply_address);
    connect(address_edit_, &QLineEdit::returnPressed, this, &MemoryDock::apply_address);
    connect(address_space_, &QComboBox::currentIndexChanged, this, [this] {
        revision_ = std::numeric_limits<std::uint64_t>::max();
        refresh();
    });
}

void MemoryDock::refresh() {
    const auto data = session_.memory(address_space_->currentIndex() == 1, start_, REQUEST_BYTES);
    if (!data.success) {
        clear();
        return;
    }
    if (data.revision == revision_) {
        return;
    }
    revision_ = data.revision;

    QString text;
    for (std::size_t row = 0; row < data.values.size(); row += BYTES_PER_ROW) {
        text += QStringLiteral("%1  ")
                    .arg(
                        static_cast<qulonglong>(data.start + row),
                        8,
                        16,
                        QLatin1Char('0'));
        QString characters;
        for (std::size_t column = 0; column < BYTES_PER_ROW; ++column) {
            const auto index = row + column;
            if (index >= data.values.size()) {
                text += QStringLiteral("   ");
                characters += QLatin1Char(' ');
            } else if (index >= data.readable.size() || data.readable[index] == 0) {
                text += QStringLiteral("?? ");
                characters += QLatin1Char('.');
            } else {
                const auto byte = data.values[index];
                text += QStringLiteral("%1 ").arg(byte, 2, 16, QLatin1Char('0'));
                characters += byte >= 0x20 && byte <= 0x7e
                    ? QChar::fromLatin1(static_cast<char>(byte))
                    : QLatin1Char('.');
            }
        }
        text += QStringLiteral(" |%1|\n").arg(characters);
    }
    text_view_->setPlainText(text);
}

void MemoryDock::clear() {
    revision_ = std::numeric_limits<std::uint64_t>::max();
    text_view_->setPlainText(QStringLiteral("No machine configured."));
}

void MemoryDock::apply_address() {
    bool valid = false;
    auto text = address_edit_->text().trimmed();
    if (text.startsWith(QStringLiteral("0x"), Qt::CaseInsensitive)) {
        text.remove(0, 2);
    }
    const auto address = text.toULongLong(&valid, 16);
    if (!valid) {
        return;
    }
    start_ = address;
    revision_ = std::numeric_limits<std::uint64_t>::max();
    refresh();
}

} // namespace se_ui
