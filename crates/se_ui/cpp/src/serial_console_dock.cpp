#include "se_ui/serial_console_dock.h"

#include <QFontDatabase>
#include <QMetaObject>
#include <QPlainTextEdit>
#include <QScrollBar>
#include <QTabWidget>
#include <QTextCursor>

#include <algorithm>
#include <utility>

namespace se_ui {

SerialConsoleDock::SerialConsoleDock(QWidget* parent)
    : QDockWidget(QStringLiteral("Serial Console"), parent)
    , serial_a_(new QPlainTextEdit(this))
    , serial_b_(new QPlainTextEdit(this))
    , serial_a_was_carriage_return_(false)
    , serial_b_was_carriage_return_(false) {
    setObjectName(QStringLiteral("SerialConsoleDock"));

    const auto fixed_font = QFontDatabase::systemFont(QFontDatabase::FixedFont);
    for (auto* editor : {serial_a_, serial_b_}) {
        editor->setReadOnly(true);
        editor->setFont(fixed_font);
        editor->setUndoRedoEnabled(false);
    }

    auto* tabs = new QTabWidget(this);
    tabs->addTab(serial_a_, QStringLiteral("Serial A"));
    tabs->addTab(serial_b_, QStringLiteral("Serial B"));
    setWidget(tabs);
}

void SerialConsoleDock::append_serial(
    const std::vector<std::uint8_t>& serial_a,
    const std::vector<std::uint8_t>& serial_b) {
    append_bytes(serial_a_, serial_a_was_carriage_return_, serial_a);
    append_bytes(serial_b_, serial_b_was_carriage_return_, serial_b);
}

void SerialConsoleDock::clear() {
    serial_a_->clear();
    serial_b_->clear();
    serial_a_was_carriage_return_ = false;
    serial_b_was_carriage_return_ = false;
}

void SerialConsoleDock::append_bytes(
    QPlainTextEdit* editor,
    bool& previous_was_carriage_return,
    const std::vector<std::uint8_t>& bytes) {
    if (bytes.empty()) {
        return;
    }

    QString text;
    text.reserve(static_cast<qsizetype>(bytes.size()));
    for (const auto byte : bytes) {
        if (byte == '\n' && previous_was_carriage_return) {
            previous_was_carriage_return = false;
            continue;
        }
        if (byte == '\r') {
            text.append(QChar::fromLatin1('\n'));
            previous_was_carriage_return = true;
            continue;
        }

        previous_was_carriage_return = false;
        text.append(QChar(byte));
    }
    if (text.isEmpty()) {
        return;
    }

    auto* scrollbar = editor->verticalScrollBar();
    const bool follow_output = scrollbar->value() == scrollbar->maximum();
    const auto selection = editor->textCursor();
    const bool restore_selection = selection.hasSelection();

    QTextCursor insertion(editor->document());
    insertion.movePosition(QTextCursor::End);
    insertion.insertText(text);

    if (restore_selection) {
        editor->setTextCursor(selection);
    }
    if (follow_output) {
        scrollbar->setValue(scrollbar->maximum());
    }
}

MachineOutputSink::MachineOutputSink(SerialConsoleDock* console)
    : console_(console)
    , delivery_scheduled_(false) {
}

void MachineOutputSink::publish_serial(
    rust::Slice<const std::uint8_t> serial_a,
    rust::Slice<const std::uint8_t> serial_b) const {
    if (serial_a.empty() && serial_b.empty()) {
        return;
    }

    bool schedule_delivery = false;
    {
        const std::lock_guard lock(mutex_);
        pending_serial_a_.insert(pending_serial_a_.end(), serial_a.begin(), serial_a.end());
        pending_serial_b_.insert(pending_serial_b_.end(), serial_b.begin(), serial_b.end());
        if (!delivery_scheduled_) {
            delivery_scheduled_ = true;
            schedule_delivery = true;
        }
    }

    if (!schedule_delivery) {
        return;
    }

    const auto self = shared_from_this();
    if (!QMetaObject::invokeMethod(
            console_, [self] { self->drain(); }, Qt::QueuedConnection)) {
        const std::lock_guard lock(mutex_);
        delivery_scheduled_ = false;
    }
}

void MachineOutputSink::drain() const {
    std::vector<std::uint8_t> serial_a;
    std::vector<std::uint8_t> serial_b;
    {
        const std::lock_guard lock(mutex_);
        serial_a.swap(pending_serial_a_);
        serial_b.swap(pending_serial_b_);
        delivery_scheduled_ = false;
    }

    console_->append_serial(serial_a, serial_b);
}

} // namespace se_ui
