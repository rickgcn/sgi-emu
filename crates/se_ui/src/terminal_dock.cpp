#include "se_ui/include/terminal_dock.h"

#include "se_ui/src/application.rs.h"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>

#include <QtCore/QCoreApplication>
#include <QtCore/QPoint>
#include <QtCore/QSignalBlocker>
#include <QtCore/QTimer>
#include <QtGui/QClipboard>
#include <QtGui/QContextMenuEvent>
#include <QtGui/QFontDatabase>
#include <QtGui/QFontMetrics>
#include <QtGui/QKeyEvent>
#include <QtGui/QMouseEvent>
#include <QtGui/QPainter>
#include <QtGui/QPalette>
#include <QtWidgets/QAbstractScrollArea>
#include <QtWidgets/QApplication>
#include <QtWidgets/QDockWidget>
#include <QtWidgets/QHBoxLayout>
#include <QtWidgets/QLabel>
#include <QtWidgets/QMainWindow>
#include <QtWidgets/QMenu>
#include <QtWidgets/QPushButton>
#include <QtWidgets/QScrollBar>
#include <QtWidgets/QTabWidget>
#include <QtWidgets/QVBoxLayout>
#include <QtWidgets/QWidget>

namespace se::ui {
namespace {

constexpr std::size_t drain_batch_size = 4'096;
constexpr int drain_interval_ms = 50;

constexpr auto terminal_text = QT_TRANSLATE_NOOP("TerminalDock", "Terminal");
constexpr auto serial_1_text = QT_TRANSLATE_NOOP("TerminalDock", "Serial 1");
constexpr auto serial_2_text = QT_TRANSLATE_NOOP("TerminalDock", "Serial 2");
constexpr auto clear_text = QT_TRANSLATE_NOOP("TerminalDock", "Clear");
constexpr auto copy_text = QT_TRANSLATE_NOOP("TerminalDock", "Copy");
constexpr auto paste_text = QT_TRANSLATE_NOOP("TerminalDock", "Paste");
constexpr auto select_all_text =
  QT_TRANSLATE_NOOP("TerminalDock", "Select All");
constexpr auto status_text = QT_TRANSLATE_NOOP(
  "TerminalDock",
  "%1 sent / %2 received / %3 dropped");

QString translate(const char* source)
{
  return QCoreApplication::translate("TerminalDock", source);
}

QString from_rust_string(const rust::String& value)
{
  return QString::fromUtf8(
    value.data(),
    static_cast<qsizetype>(value.size()));
}

QColor indexed_color(std::uint8_t index)
{
  static const std::array<QColor, 16> ansi = {
    QColor("#1C1C1C"), QColor("#CC5555"), QColor("#55AA55"),
    QColor("#CDCD55"), QColor("#5555CC"), QColor("#CC55CC"),
    QColor("#55CCCC"), QColor("#D8D8D8"), QColor("#666666"),
    QColor("#FF7777"), QColor("#77DD77"), QColor("#FFFF77"),
    QColor("#7777FF"), QColor("#FF77FF"), QColor("#77FFFF"),
    QColor("#FFFFFF")
  };
  if (index < ansi.size()) {
    return ansi[index];
  }
  if (index < 232) {
    const auto value = static_cast<int>(index) - 16;
    const auto channel = [](int component) {
      return component == 0 ? 0 : 55 + component * 40;
    };
    return QColor(
      channel(value / 36),
      channel((value / 6) % 6),
      channel(value % 6));
  }
  const auto gray = 8 + (static_cast<int>(index) - 232) * 10;
  return QColor(gray, gray, gray);
}

QColor terminal_color(
  UiTerminalColorKind kind,
  std::uint8_t index,
  std::uint8_t red,
  std::uint8_t green,
  std::uint8_t blue,
  const QColor& fallback)
{
  switch (kind) {
  case UiTerminalColorKind::Default:
    return fallback;
  case UiTerminalColorKind::Indexed:
    return indexed_color(index);
  case UiTerminalColorKind::Rgb:
    return QColor(red, green, blue);
  }
  return fallback;
}

class TerminalView final : public QAbstractScrollArea
{
public:
  TerminalView(
    const EmulationController& controller,
    TerminalModel& model,
    UiSerialPort port,
    QWidget* parent)
    : QAbstractScrollArea(parent)
    , controller_(controller)
    , model_(model)
    , port_(port)
    , font_(QFontDatabase::systemFont(QFontDatabase::FixedFont))
  {
    font_.setStyleHint(QFont::Monospace);
    setFont(font_);
    setFocusPolicy(Qt::StrongFocus);
    setMouseTracking(true);
    setHorizontalScrollBarPolicy(Qt::ScrollBarAsNeeded);
    setVerticalScrollBarPolicy(Qt::ScrollBarAsNeeded);
    connect(
      verticalScrollBar(),
      &QScrollBar::valueChanged,
      this,
      [this](int value) {
        if (updating_scrollbar_) {
          return;
        }
        const auto offset = verticalScrollBar()->maximum() - value;
        model_.set_scrollback(port_, static_cast<std::size_t>(offset));
        refresh();
      });
    refresh();
  }

  void refresh()
  {
    snapshot_ = model_.snapshot(port_);
    QFontMetrics metrics(font_);
    cell_width_ = std::max(1, metrics.horizontalAdvance(QLatin1Char('M')));
    cell_height_ = std::max(1, metrics.height());
    ascent_ = metrics.ascent();
    const auto content_width = static_cast<int>(snapshot_.columns) * cell_width_;
    horizontalScrollBar()->setRange(
      0,
      std::max(0, content_width - viewport()->width()));
    horizontalScrollBar()->setPageStep(viewport()->width());

    updating_scrollbar_ = true;
    verticalScrollBar()->setRange(
      0,
      static_cast<int>(snapshot_.maximum_scrollback));
    verticalScrollBar()->setPageStep(static_cast<int>(snapshot_.rows));
    verticalScrollBar()->setValue(
      verticalScrollBar()->maximum() - static_cast<int>(snapshot_.scrollback));
    updating_scrollbar_ = false;

    if (snapshot_.bell_count > bell_count_) {
      QApplication::beep();
      bell_count_ = snapshot_.bell_count;
    }
    viewport()->update();
  }

  void clearTerminal()
  {
    model_.clear(port_);
    selection_active_ = false;
    refresh();
  }

  void copySelection()
  {
    if (!selection_active_) {
      return;
    }
    auto start = selection_start_;
    auto end = selection_end_;
    if (cell_before(end, start)) {
      std::swap(start, end);
    }
    const auto text = model_.selected_text(
      port_,
      static_cast<std::uint16_t>(start.y()),
      static_cast<std::uint16_t>(start.x()),
      static_cast<std::uint16_t>(end.y()),
      static_cast<std::uint16_t>(std::min(
        end.x() + 1,
        static_cast<int>(snapshot_.columns))));
    QApplication::clipboard()->setText(from_rust_string(text));
  }

  void selectAll()
  {
    selection_start_ = QPoint(0, 0);
    selection_end_ = QPoint(
      static_cast<int>(snapshot_.columns) - 1,
      static_cast<int>(snapshot_.rows) - 1);
    selection_active_ = true;
    viewport()->update();
  }

  void pasteClipboard()
  {
    const auto text = QApplication::clipboard()->text().toUtf8();
    const auto encoded = model_.encode_paste(
      port_,
      rust::Str(text.constData(), static_cast<std::size_t>(text.size())));
    sendBytes(encoded.data(), encoded.size());
  }

protected:
  void paintEvent(QPaintEvent*) override
  {
    QPainter painter(viewport());
    const auto default_background = palette().color(QPalette::Base);
    const auto default_foreground = palette().color(QPalette::Text);
    painter.fillRect(viewport()->rect(), default_background);
    const auto x_offset = -horizontalScrollBar()->value();

    for (std::uint16_t row = 0; row < snapshot_.rows; ++row) {
      for (std::uint16_t column = 0; column < snapshot_.columns; ++column) {
        const auto index = static_cast<std::size_t>(row) * snapshot_.columns + column;
        const auto& cell = snapshot_.cells[index];
        if (cell.wide_continuation) {
          continue;
        }
        auto foreground = terminal_color(
          cell.foreground_kind,
          cell.foreground_index,
          cell.foreground_red,
          cell.foreground_green,
          cell.foreground_blue,
          default_foreground);
        auto background = terminal_color(
          cell.background_kind,
          cell.background_index,
          cell.background_red,
          cell.background_green,
          cell.background_blue,
          default_background);
        if (cell.inverse) {
          std::swap(foreground, background);
        }
        const QRect rectangle(
          x_offset + static_cast<int>(column) * cell_width_,
          static_cast<int>(row) * cell_height_,
          cell_width_ * (cell.wide ? 2 : 1),
          cell_height_);
        if (selected(column, row)) {
          background = palette().color(QPalette::Highlight);
          foreground = palette().color(QPalette::HighlightedText);
        }
        painter.fillRect(rectangle, background);
        if (cell.dim) {
          foreground.setAlpha(150);
        }
        QFont cell_font = font_;
        cell_font.setBold(cell.bold);
        cell_font.setItalic(cell.italic);
        cell_font.setUnderline(cell.underline);
        painter.setFont(cell_font);
        painter.setPen(foreground);
        painter.drawText(
          rectangle.left(),
          rectangle.top() + ascent_,
          from_rust_string(cell.text));
      }
    }

    if (snapshot_.cursor_visible && hasFocus()) {
      const QRect cursor(
        x_offset + static_cast<int>(snapshot_.cursor_column) * cell_width_,
        static_cast<int>(snapshot_.cursor_row) * cell_height_,
        cell_width_,
        cell_height_);
      painter.fillRect(cursor, palette().color(QPalette::Highlight));
      const auto index = static_cast<std::size_t>(snapshot_.cursor_row)
                           * snapshot_.columns
                         + snapshot_.cursor_column;
      if (index < snapshot_.cells.size()) {
        painter.setPen(palette().color(QPalette::HighlightedText));
        painter.setFont(font_);
        painter.drawText(
          cursor.left(),
          cursor.top() + ascent_,
          from_rust_string(snapshot_.cells[index].text));
      }
    }
  }

  void resizeEvent(QResizeEvent* event) override
  {
    QAbstractScrollArea::resizeEvent(event);
    refresh();
  }

  void keyPressEvent(QKeyEvent* event) override
  {
    const auto modifiers = event->modifiers();
    const auto copy_shortcut = modifiers.testFlag(Qt::ControlModifier)
                                 && modifiers.testFlag(Qt::ShiftModifier)
                                 && event->key() == Qt::Key_C;
    const auto paste_shortcut = modifiers.testFlag(Qt::ControlModifier)
                                  && modifiers.testFlag(Qt::ShiftModifier)
                                  && event->key() == Qt::Key_V;
#ifdef Q_OS_MACOS
    if (modifiers.testFlag(Qt::MetaModifier) && event->key() == Qt::Key_C) {
      copySelection();
      return;
    }
    if (modifiers.testFlag(Qt::MetaModifier) && event->key() == Qt::Key_V) {
      pasteClipboard();
      return;
    }
#endif
    if (copy_shortcut) {
      copySelection();
      return;
    }
    if (paste_shortcut) {
      pasteClipboard();
      return;
    }
    if (event->key() == Qt::Key_End && snapshot_.scrollback != 0
        && modifiers == Qt::NoModifier) {
      model_.set_scrollback(port_, 0);
      refresh();
      return;
    }

    const auto key = terminalKey(event->key());
    auto text = event->text().toUtf8();
    if (modifiers.testFlag(Qt::ControlModifier)
        && event->key() >= Qt::Key_A && event->key() <= Qt::Key_Z) {
      text = QByteArray(1, static_cast<char>('a' + event->key() - Qt::Key_A));
    }
    const auto encoded = model_.encode_key(
      port_,
      key,
      rust::Str(text.constData(), static_cast<std::size_t>(text.size())),
      modifiers.testFlag(Qt::ControlModifier),
      modifiers.testFlag(Qt::AltModifier));
    if (encoded.empty()) {
      QAbstractScrollArea::keyPressEvent(event);
      return;
    }
    sendBytes(encoded.data(), encoded.size());
    event->accept();
  }

  void mousePressEvent(QMouseEvent* event) override
  {
    if (event->button() != Qt::LeftButton) {
      return;
    }
    selection_start_ = cellAt(event->position().toPoint());
    selection_end_ = selection_start_;
    selection_active_ = true;
    viewport()->update();
  }

  void mouseMoveEvent(QMouseEvent* event) override
  {
    if (!selection_active_ || !event->buttons().testFlag(Qt::LeftButton)) {
      return;
    }
    selection_end_ = cellAt(event->position().toPoint());
    viewport()->update();
  }

  void contextMenuEvent(QContextMenuEvent* event) override
  {
    QMenu menu(this);
    auto* copy = menu.addAction(translate(copy_text));
    copy->setEnabled(selection_active_);
    connect(copy, &QAction::triggered, this, [this] { copySelection(); });
    auto* paste = menu.addAction(translate(paste_text));
    connect(paste, &QAction::triggered, this, [this] { pasteClipboard(); });
    menu.addSeparator();
    auto* select_all = menu.addAction(translate(select_all_text));
    connect(select_all, &QAction::triggered, this, [this] { selectAll(); });
    auto* clear = menu.addAction(translate(clear_text));
    connect(clear, &QAction::triggered, this, [this] { clearTerminal(); });
    menu.exec(event->globalPos());
  }

private:
  static UiTerminalKey terminalKey(int key)
  {
    switch (key) {
    case Qt::Key_Return:
    case Qt::Key_Enter:
      return UiTerminalKey::Enter;
    case Qt::Key_Backspace:
      return UiTerminalKey::Backspace;
    case Qt::Key_Tab:
    case Qt::Key_Backtab:
      return UiTerminalKey::Tab;
    case Qt::Key_Escape:
      return UiTerminalKey::Escape;
    case Qt::Key_Up:
      return UiTerminalKey::Up;
    case Qt::Key_Down:
      return UiTerminalKey::Down;
    case Qt::Key_Right:
      return UiTerminalKey::Right;
    case Qt::Key_Left:
      return UiTerminalKey::Left;
    case Qt::Key_Home:
      return UiTerminalKey::Home;
    case Qt::Key_End:
      return UiTerminalKey::End;
    case Qt::Key_Insert:
      return UiTerminalKey::Insert;
    case Qt::Key_Delete:
      return UiTerminalKey::Delete;
    case Qt::Key_PageUp:
      return UiTerminalKey::PageUp;
    case Qt::Key_PageDown:
      return UiTerminalKey::PageDown;
    default:
      return UiTerminalKey::Text;
    }
  }

  void sendBytes(const std::uint8_t* data, std::size_t size)
  {
    std::size_t offset = 0;
    while (offset < size) {
      const auto count = std::min(drain_batch_size, size - offset);
      const auto status = controller_.submit_terminal_input(
        port_,
        rust::Slice<const std::uint8_t>(data + offset, count));
      if (status != TerminalInputStatus::Accepted) {
        QApplication::beep();
        return;
      }
      offset += count;
    }
  }

  QPoint cellAt(const QPoint& point) const
  {
    const auto column = std::clamp(
      (point.x() + horizontalScrollBar()->value()) / cell_width_,
      0,
      static_cast<int>(snapshot_.columns) - 1);
    const auto row = std::clamp(
      point.y() / cell_height_,
      0,
      static_cast<int>(snapshot_.rows) - 1);
    return QPoint(column, row);
  }

  static bool cell_before(const QPoint& left, const QPoint& right)
  {
    return left.y() < right.y()
           || (left.y() == right.y() && left.x() < right.x());
  }

  bool selected(std::uint16_t column, std::uint16_t row) const
  {
    if (!selection_active_) {
      return false;
    }
    auto start = selection_start_;
    auto end = selection_end_;
    if (cell_before(end, start)) {
      std::swap(start, end);
    }
    const QPoint cell(column, row);
    return !cell_before(cell, start) && !cell_before(end, cell);
  }

  const EmulationController& controller_;
  TerminalModel& model_;
  UiSerialPort port_;
  QFont font_;
  UiTerminalSnapshot snapshot_;
  int cell_width_ = 1;
  int cell_height_ = 1;
  int ascent_ = 1;
  bool updating_scrollbar_ = false;
  bool selection_active_ = false;
  QPoint selection_start_;
  QPoint selection_end_;
  std::uint64_t bell_count_ = 0;
};

class TerminalPane final : public QWidget
{
public:
  TerminalPane(const EmulationController& controller, QWidget* parent)
    : QWidget(parent)
    , controller_(controller)
    , model_(new_terminal_model())
  {
    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(4, 4, 4, 4);
    auto* controls = new QHBoxLayout;
    controls->addStretch(1);
    auto* clear = new QPushButton(translate(clear_text), this);
    controls->addWidget(clear);
    layout->addLayout(controls);

    tabs_ = new QTabWidget(this);
    serial_1_ = new TerminalView(
      controller_, *model_, UiSerialPort::Serial1, tabs_);
    serial_2_ = new TerminalView(
      controller_, *model_, UiSerialPort::Serial2, tabs_);
    tabs_->addTab(serial_1_, translate(serial_1_text));
    tabs_->addTab(serial_2_, translate(serial_2_text));
    layout->addWidget(tabs_, 1);

    status_ = new QLabel(this);
    layout->addWidget(status_);

    connect(clear, &QPushButton::clicked, this, [this] {
      activeView()->clearTerminal();
    });
    connect(tabs_, &QTabWidget::currentChanged, this, [this] {
      activeView()->setFocus();
      updateStatus();
    });

    auto* timer = new QTimer(this);
    timer->setInterval(drain_interval_ms);
    connect(timer, &QTimer::timeout, this, [this] { updateFromController(); });
    timer->start();
    updateFromController();
  }

private:
  TerminalView* activeView() const
  {
    return tabs_->currentIndex() == 0 ? serial_1_ : serial_2_;
  }

  UiSerialPort activePort() const
  {
    return tabs_->currentIndex() == 0 ? UiSerialPort::Serial1
                                      : UiSerialPort::Serial2;
  }

  void updateFromController()
  {
    const auto state = controller_.snapshot();
    if (session_id_ != state.session_id) {
      session_id_ = state.session_id;
      model_->clear_all();
    }
    auto chunks = controller_.drain_terminal_output(drain_batch_size);
    for (const auto& chunk : chunks) {
      if (chunk.session_id != session_id_) {
        continue;
      }
      model_->process_output(
        chunk.port,
        rust::Slice<const std::uint8_t>(chunk.bytes.data(), chunk.bytes.size()));
    }
    serial_1_->refresh();
    serial_2_->refresh();
    updateStatus();
  }

  void updateStatus()
  {
    const auto stats = controller_.terminal_io_stats(activePort());
    status_->setText(translate(status_text).arg(
      stats.sent,
      0,
      10).arg(stats.received, 0, 10).arg(stats.dropped, 0, 10));
  }

  const EmulationController& controller_;
  rust::Box<TerminalModel> model_;
  QTabWidget* tabs_ = nullptr;
  TerminalView* serial_1_ = nullptr;
  TerminalView* serial_2_ = nullptr;
  QLabel* status_ = nullptr;
  std::uint64_t session_id_ = 0;
};

}

QDockWidget* create_terminal_dock(
  QMainWindow* parent,
  const EmulationController& controller)
{
  auto* dock = new QDockWidget(translate(terminal_text), parent);
  dock->setObjectName(QStringLiteral("terminalDock"));
  dock->setAllowedAreas(Qt::AllDockWidgetAreas);
  dock->setWidget(new TerminalPane(controller, dock));
  return dock;
}

}
