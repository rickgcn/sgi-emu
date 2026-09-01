#pragma once

#include "rust/cxx.h"

#include <QDockWidget>

#include <cstdint>
#include <memory>
#include <mutex>
#include <vector>

class QPlainTextEdit;

namespace se_ui {

class SerialConsoleDock final : public QDockWidget {
public:
    explicit SerialConsoleDock(QWidget* parent = nullptr);

    void append_serial(
        const std::vector<std::uint8_t>& serial_a,
        const std::vector<std::uint8_t>& serial_b);
    void clear();

private:
    static void append_bytes(
        QPlainTextEdit* editor,
        bool& previous_was_carriage_return,
        const std::vector<std::uint8_t>& bytes);

    QPlainTextEdit* serial_a_;
    QPlainTextEdit* serial_b_;
    bool serial_a_was_carriage_return_;
    bool serial_b_was_carriage_return_;
};

class MachineOutputSink final : public std::enable_shared_from_this<MachineOutputSink> {
public:
    explicit MachineOutputSink(SerialConsoleDock* console);

    void publish_serial(
        rust::Slice<const std::uint8_t> serial_a,
        rust::Slice<const std::uint8_t> serial_b) const;

private:
    void drain() const;

    SerialConsoleDock* console_;
    mutable std::mutex mutex_;
    mutable std::vector<std::uint8_t> pending_serial_a_;
    mutable std::vector<std::uint8_t> pending_serial_b_;
    mutable bool delivery_scheduled_;
};

} // namespace se_ui
