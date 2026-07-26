#include "se_ui/include/tracing_dock.h"

#include "se_ui/src/tracing.rs.h"

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <utility>
#include <vector>

#include <QtCore/QAbstractTableModel>
#include <QtCore/QCoreApplication>
#include <QtCore/QItemSelectionModel>
#include <QtCore/QModelIndex>
#include <QtCore/QSortFilterProxyModel>
#include <QtCore/QStringList>
#include <QtCore/QTimer>
#include <QtCore/QtGlobal>
#include <QtGui/QAction>
#include <QtGui/QClipboard>
#include <QtGui/QColor>
#include <QtGui/QKeySequence>
#include <QtWidgets/QAbstractItemView>
#include <QtWidgets/QApplication>
#include <QtWidgets/QCheckBox>
#include <QtWidgets/QComboBox>
#include <QtWidgets/QDockWidget>
#include <QtWidgets/QHBoxLayout>
#include <QtWidgets/QHeaderView>
#include <QtWidgets/QLabel>
#include <QtWidgets/QLineEdit>
#include <QtWidgets/QMainWindow>
#include <QtWidgets/QMenu>
#include <QtWidgets/QPushButton>
#include <QtWidgets/QScrollBar>
#include <QtWidgets/QSplitter>
#include <QtWidgets/QStackedWidget>
#include <QtWidgets/QTableView>
#include <QtWidgets/QTreeWidget>
#include <QtWidgets/QVBoxLayout>
#include <QtWidgets/QWidget>

namespace se::ui {
namespace {

constexpr std::size_t drain_batch_size = 4'096;
constexpr std::size_t record_capacity = 100'000;
constexpr int drain_interval_ms = 50;
constexpr int search_delay_ms = 150;

constexpr auto tracing_text = QT_TRANSLATE_NOOP("TracingDock", "Tracing");
constexpr auto error_text = QT_TRANSLATE_NOOP("TracingDock", "Error");
constexpr auto warn_text = QT_TRANSLATE_NOOP("TracingDock", "Warn");
constexpr auto info_text = QT_TRANSLATE_NOOP("TracingDock", "Info");
constexpr auto debug_text = QT_TRANSLATE_NOOP("TracingDock", "Debug");
constexpr auto trace_text = QT_TRANSLATE_NOOP("TracingDock", "Trace");
constexpr auto runtime_text = QT_TRANSLATE_NOOP("TracingDock", "Runtime");
constexpr auto scheduler_text = QT_TRANSLATE_NOOP("TracingDock", "Scheduler");
constexpr auto component_text = QT_TRANSLATE_NOOP("TracingDock", "Component");
constexpr auto component_id_text =
  QT_TRANSLATE_NOOP("TracingDock", "Component %1");
constexpr auto sequence_text = QT_TRANSLATE_NOOP("TracingDock", "Sequence");
constexpr auto sim_time_text = QT_TRANSLATE_NOOP("TracingDock", "Sim Time");
constexpr auto level_text = QT_TRANSLATE_NOOP("TracingDock", "Level");
constexpr auto source_text = QT_TRANSLATE_NOOP("TracingDock", "Source");
constexpr auto target_text = QT_TRANSLATE_NOOP("TracingDock", "Target");
constexpr auto event_text = QT_TRANSLATE_NOOP("TracingDock", "Event");
constexpr auto summary_text = QT_TRANSLATE_NOOP("TracingDock", "Summary");
constexpr auto search_text =
  QT_TRANSLATE_NOOP("TracingDock", "Search trace records");
constexpr auto all_levels_text =
  QT_TRANSLATE_NOOP("TracingDock", "All Levels");
constexpr auto all_sources_text =
  QT_TRANSLATE_NOOP("TracingDock", "All Sources");
constexpr auto all_targets_text =
  QT_TRANSLATE_NOOP("TracingDock", "All Targets");
constexpr auto follow_live_text =
  QT_TRANSLATE_NOOP("TracingDock", "Follow Live");
constexpr auto capture_text = QT_TRANSLATE_NOOP("TracingDock", "Capture");
constexpr auto capture_scheduler_text =
  QT_TRANSLATE_NOOP("TracingDock", "Capture Scheduler");
constexpr auto clear_text = QT_TRANSLATE_NOOP("TracingDock", "Clear");
constexpr auto no_records_text =
  QT_TRANSLATE_NOOP("TracingDock", "No trace records");
constexpr auto field_text = QT_TRANSLATE_NOOP("TracingDock", "Field");
constexpr auto type_text = QT_TRANSLATE_NOOP("TracingDock", "Type");
constexpr auto value_text = QT_TRANSLATE_NOOP("TracingDock", "Value");
constexpr auto copy_row_text = QT_TRANSLATE_NOOP("TracingDock", "Copy Row");
constexpr auto copy_field_text =
  QT_TRANSLATE_NOOP("TracingDock", "Copy Field");
constexpr auto status_text =
  QT_TRANSLATE_NOOP("TracingDock", "%1 shown / %2 captured / %3 dropped");

QString translate(const char* source)
{
  return QCoreApplication::translate("TracingDock", source);
}

QString from_rust_string(const rust::String& value)
{
  return QString::fromUtf8(
    value.data(),
    static_cast<qsizetype>(value.size()));
}

QString level_name(UiTraceLevel level)
{
  switch (level) {
  case UiTraceLevel::Error:
    return translate(error_text);
  case UiTraceLevel::Warn:
    return translate(warn_text);
  case UiTraceLevel::Info:
    return translate(info_text);
  case UiTraceLevel::Debug:
    return translate(debug_text);
  case UiTraceLevel::Trace:
    return translate(trace_text);
  }
  return {};
}

QColor level_color(UiTraceLevel level)
{
  switch (level) {
  case UiTraceLevel::Error:
    return QColor(QStringLiteral("#D94A4A"));
  case UiTraceLevel::Warn:
    return QColor(QStringLiteral("#D99A2B"));
  case UiTraceLevel::Info:
    return QColor(QStringLiteral("#17A9B8"));
  case UiTraceLevel::Debug:
    return QColor(QStringLiteral("#8B67C8"));
  case UiTraceLevel::Trace:
    return QColor(QStringLiteral("#7C8790"));
  }
  return {};
}

QString source_name(UiTraceSourceKind kind, std::uint64_t component)
{
  switch (kind) {
  case UiTraceSourceKind::Runtime:
    return translate(runtime_text);
  case UiTraceSourceKind::Scheduler:
    return translate(scheduler_text);
  case UiTraceSourceKind::Component:
    return translate(component_id_text).arg(component);
  }
  return {};
}

struct TraceField
{
  QString key;
  QString type;
  QString value;
};

struct TraceRecord
{
  std::uint64_t sequence = 0;
  std::uint64_t time = 0;
  UiTraceLevel level = UiTraceLevel::Trace;
  UiTraceSourceKind source_kind = UiTraceSourceKind::Runtime;
  QString source;
  QString target;
  QString event;
  QString summary;
  QString searchable;
  std::vector<TraceField> fields;
};

TraceField make_field(const UiTraceField& input)
{
  TraceField field;
  field.key = from_rust_string(input.key);

  switch (input.kind) {
  case UiTraceValueKind::Bool:
    field.type = QStringLiteral("Bool");
    field.value = input.bool_value ? QStringLiteral("true") : QStringLiteral("false");
    break;
  case UiTraceValueKind::U64:
    field.type = QStringLiteral("U64");
    field.value = QString::number(input.unsigned_value);
    break;
  case UiTraceValueKind::I64:
    field.type = QStringLiteral("I64");
    field.value = QString::number(input.signed_value);
    break;
  case UiTraceValueKind::Hex64:
    field.type = QStringLiteral("Hex64");
    field.value = QStringLiteral("0x%1").arg(
      input.unsigned_value,
      16,
      16,
      QLatin1Char('0'));
    field.value = QStringLiteral("0x") + field.value.mid(2).toUpper();
    break;
  case UiTraceValueKind::Str:
    field.type = QStringLiteral("Str");
    field.value = from_rust_string(input.string_value);
    break;
  }
  return field;
}

TraceRecord make_record(const UiTraceRecord& input)
{
  TraceRecord record;
  record.sequence = input.sequence;
  record.time = input.time;
  record.level = input.level;
  record.source_kind = input.source_kind;
  record.source = source_name(input.source_kind, input.source_component);
  record.target = from_rust_string(input.target);
  record.event = from_rust_string(input.event);
  record.fields.reserve(input.fields.size());

  QStringList summary;
  for (const auto& input_field : input.fields) {
    auto field = make_field(input_field);
    summary.push_back(QStringLiteral("%1=%2").arg(field.key, field.value));
    record.fields.push_back(std::move(field));
  }
  record.summary = summary.join(QStringLiteral(", "));
  record.searchable = QStringLiteral("%1 %2 %3 %4 %5 %6 %7")
                        .arg(record.sequence)
                        .arg(record.time)
                        .arg(level_name(record.level), record.source)
                        .arg(record.target, record.event, record.summary);
  return record;
}

class TraceTableModel final : public QAbstractTableModel
{
public:
  enum Column {
    Sequence,
    SimTime,
    Level,
    Source,
    Target,
    Event,
    Summary,
    ColumnCount,
  };

  enum Role {
    SearchRole = Qt::UserRole + 1,
    LevelRole,
    SourceKindRole,
    TargetRole,
  };

  explicit TraceTableModel(QObject* parent)
    : QAbstractTableModel(parent)
  {
  }

  int rowCount(const QModelIndex& parent = {}) const override
  {
    return parent.isValid() ? 0 : static_cast<int>(records_.size());
  }

  int columnCount(const QModelIndex& parent = {}) const override
  {
    return parent.isValid() ? 0 : ColumnCount;
  }

  QVariant data(const QModelIndex& index, int role) const override
  {
    if (!index.isValid() || index.row() < 0
        || static_cast<std::size_t>(index.row()) >= records_.size()) {
      return {};
    }

    const auto& record = records_[static_cast<std::size_t>(index.row())];
    if (role == Qt::ForegroundRole && index.column() == Level) {
      return level_color(record.level);
    }
    if (role == SearchRole) {
      return record.searchable;
    }
    if (role == LevelRole) {
      return static_cast<int>(record.level);
    }
    if (role == SourceKindRole) {
      return static_cast<int>(record.source_kind);
    }
    if (role == TargetRole) {
      return record.target;
    }
    if (role != Qt::DisplayRole) {
      return {};
    }

    switch (index.column()) {
    case Sequence:
      return QString::number(record.sequence);
    case SimTime:
      return QString::number(record.time);
    case Level:
      return level_name(record.level);
    case Source:
      return record.source;
    case Target:
      return record.target;
    case Event:
      return record.event;
    case Summary:
      return record.summary;
    default:
      return {};
    }
  }

  QVariant headerData(
    int section,
    Qt::Orientation orientation,
    int role) const override
  {
    if (orientation != Qt::Horizontal || role != Qt::DisplayRole) {
      return {};
    }
    switch (section) {
    case Sequence:
      return translate(sequence_text);
    case SimTime:
      return translate(sim_time_text);
    case Level:
      return translate(level_text);
    case Source:
      return translate(source_text);
    case Target:
      return translate(target_text);
    case Event:
      return translate(event_text);
    case Summary:
      return translate(summary_text);
    default:
      return {};
    }
  }

  std::uint64_t append(std::vector<TraceRecord> records)
  {
    if (records.empty()) {
      return 0;
    }

    std::uint64_t evicted = 0;
    const auto overflow = records_.size() + records.size() > record_capacity
      ? records_.size() + records.size() - record_capacity
      : 0;
    if (overflow > 0) {
      beginRemoveRows({}, 0, static_cast<int>(overflow - 1));
      records_.erase(
        records_.begin(),
        records_.begin() + static_cast<std::ptrdiff_t>(overflow));
      endRemoveRows();
      evicted = overflow;
    }

    const auto first = static_cast<int>(records_.size());
    const auto last = first + static_cast<int>(records.size()) - 1;
    beginInsertRows({}, first, last);
    for (auto& record : records) {
      records_.push_back(std::move(record));
    }
    endInsertRows();
    return evicted;
  }

  void clear()
  {
    if (records_.empty()) {
      return;
    }
    beginResetModel();
    records_.clear();
    endResetModel();
  }

  const TraceRecord* record_at(int row) const
  {
    if (row < 0 || static_cast<std::size_t>(row) >= records_.size()) {
      return nullptr;
    }
    return &records_[static_cast<std::size_t>(row)];
  }

private:
  std::deque<TraceRecord> records_;
};

class TraceFilterModel final : public QSortFilterProxyModel
{
public:
  explicit TraceFilterModel(QObject* parent)
    : QSortFilterProxyModel(parent)
  {
    setDynamicSortFilter(true);
    setSortCaseSensitivity(Qt::CaseInsensitive);
  }

  void set_search(QString search)
  {
    update_filter(search_, std::move(search));
  }

  void set_level(int level)
  {
    update_filter(level_, level);
  }

  void set_source(int source)
  {
    update_filter(source_, source);
  }

  void set_target(QString target)
  {
    update_filter(target_, std::move(target));
  }

protected:
  bool filterAcceptsRow(int row, const QModelIndex& parent) const override
  {
    const auto index = sourceModel()->index(row, 0, parent);
    if (level_ >= 0
        && index.data(TraceTableModel::LevelRole).toInt() != level_) {
      return false;
    }
    if (source_ >= 0
        && index.data(TraceTableModel::SourceKindRole).toInt() != source_) {
      return false;
    }
    if (!target_.isEmpty()
        && index.data(TraceTableModel::TargetRole).toString() != target_) {
      return false;
    }
    return search_.isEmpty()
      || index.data(TraceTableModel::SearchRole)
           .toString()
           .contains(search_, Qt::CaseInsensitive);
  }

  bool lessThan(
    const QModelIndex& left,
    const QModelIndex& right) const override
  {
    if (left.column() == TraceTableModel::Sequence
        || left.column() == TraceTableModel::SimTime) {
      return left.data().toString().toULongLong()
        < right.data().toString().toULongLong();
    }
    return QSortFilterProxyModel::lessThan(left, right);
  }

private:
  template<typename T>
  void update_filter(T& filter, T value)
  {
#if QT_VERSION >= QT_VERSION_CHECK(6, 9, 0)
    beginFilterChange();
#endif
    filter = std::move(value);
#if QT_VERSION >= QT_VERSION_CHECK(6, 10, 0)
    endFilterChange(QSortFilterProxyModel::Direction::Rows);
#else
    invalidateRowsFilter();
#endif
  }

  QString search_;
  QString target_;
  int level_ = -1;
  int source_ = -1;
};

class TracingPanel final : public QWidget
{
public:
  explicit TracingPanel(QWidget* parent)
    : QWidget(parent)
  {
    model_ = new TraceTableModel(this);
    filter_ = new TraceFilterModel(this);
    filter_->setSourceModel(model_);

    build_controls();
    build_views();

    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(6, 6, 6, 6);
    layout->setSpacing(4);
    layout->addLayout(controls_);
    layout->addWidget(splitter_, 1);
    layout->addWidget(status_);

    connect_controls();
    trace_session_ = trace_stats().session;
    update_empty_state();
    update_status();

    auto* drain_timer = new QTimer(this);
    drain_timer->setInterval(drain_interval_ms);
    connect(drain_timer, &QTimer::timeout, this, [this] { drain_records(); });
    drain_timer->start();
  }

private:
  void build_controls()
  {
    controls_ = new QHBoxLayout();
    controls_->setContentsMargins(0, 0, 0, 0);

    search_ = new QLineEdit(this);
    search_->setClearButtonEnabled(true);
    search_->setPlaceholderText(translate(search_text));
    controls_->addWidget(search_, 1);

    level_ = new QComboBox(this);
    level_->addItem(
      translate(all_levels_text),
      -1);
    level_->addItem(level_name(UiTraceLevel::Error), static_cast<int>(UiTraceLevel::Error));
    level_->addItem(level_name(UiTraceLevel::Warn), static_cast<int>(UiTraceLevel::Warn));
    level_->addItem(level_name(UiTraceLevel::Info), static_cast<int>(UiTraceLevel::Info));
    level_->addItem(level_name(UiTraceLevel::Debug), static_cast<int>(UiTraceLevel::Debug));
    level_->addItem(level_name(UiTraceLevel::Trace), static_cast<int>(UiTraceLevel::Trace));
    controls_->addWidget(level_);

    source_ = new QComboBox(this);
    source_->addItem(
      translate(all_sources_text),
      -1);
    source_->addItem(
      translate(runtime_text),
      static_cast<int>(UiTraceSourceKind::Runtime));
    source_->addItem(
      translate(scheduler_text),
      static_cast<int>(UiTraceSourceKind::Scheduler));
    source_->addItem(
      translate(component_text),
      static_cast<int>(UiTraceSourceKind::Component));
    controls_->addWidget(source_);

    target_ = new QComboBox(this);
    target_->addItem(
      translate(all_targets_text),
      QString());
    controls_->addWidget(target_);

    capture_ = new QCheckBox(translate(capture_text), this);
    capture_->setChecked(false);
    capture_->setMinimumWidth(capture_->sizeHint().width());
    set_trace_capture_enabled(false);
    controls_->addWidget(capture_);
    controls_->addSpacing(10);

    scheduler_capture_ = new QCheckBox(translate(capture_scheduler_text), this);
    scheduler_capture_->setChecked(false);
    scheduler_capture_->setMinimumWidth(scheduler_capture_->sizeHint().width());
    set_scheduler_trace_capture_enabled(false);
    controls_->addWidget(scheduler_capture_);
    controls_->addSpacing(10);

    follow_ = new QCheckBox(translate(follow_live_text), this);
    follow_->setChecked(true);
    follow_->setMinimumWidth(follow_->sizeHint().width());
    controls_->addWidget(follow_);

    clear_ = new QPushButton(
      translate(clear_text),
      this);
    clear_->setFixedHeight(level_->sizeHint().height());
    auto* clear_slot = new QWidget(this);
    auto* clear_layout = new QVBoxLayout(clear_slot);
    clear_layout->setContentsMargins(0, 1, 0, 0);
    clear_layout->setSpacing(0);
    clear_layout->addWidget(clear_);
    controls_->insertWidget(1, clear_slot, 0, Qt::AlignTop);
  }

  void build_views()
  {
    table_ = new QTableView(this);
    table_->setModel(filter_);
    table_->setAlternatingRowColors(true);
    table_->setSelectionBehavior(QAbstractItemView::SelectRows);
    table_->setSelectionMode(QAbstractItemView::SingleSelection);
    table_->setSortingEnabled(true);
    table_->setContextMenuPolicy(Qt::CustomContextMenu);
    table_->verticalHeader()->setVisible(false);
    table_->horizontalHeader()->setStretchLastSection(true);
    table_->horizontalHeader()->setSectionResizeMode(QHeaderView::Interactive);
    table_->setColumnWidth(TraceTableModel::Sequence, 90);
    table_->setColumnWidth(TraceTableModel::SimTime, 100);
    table_->setColumnWidth(TraceTableModel::Level, 75);
    table_->setColumnWidth(TraceTableModel::Source, 130);
    table_->setColumnWidth(TraceTableModel::Target, 170);
    table_->setColumnWidth(TraceTableModel::Event, 170);

    empty_ = new QLabel(
      translate(no_records_text),
      this);
    empty_->setAlignment(Qt::AlignCenter);
    empty_->setEnabled(false);

    table_stack_ = new QStackedWidget(this);
    table_stack_->addWidget(table_);
    table_stack_->addWidget(empty_);

    details_ = new QTreeWidget(this);
    details_->setHeaderLabels(
      { translate(field_text), translate(type_text), translate(value_text) });
    details_->setRootIsDecorated(false);
    details_->setAlternatingRowColors(true);
    details_->setContextMenuPolicy(Qt::CustomContextMenu);
    details_->header()->setStretchLastSection(true);

    splitter_ = new QSplitter(Qt::Vertical, this);
    splitter_->addWidget(table_stack_);
    splitter_->addWidget(details_);
    splitter_->setSizes({ 230, 90 });

    status_ = new QLabel(this);
  }

  void connect_controls()
  {
    search_timer_ = new QTimer(this);
    search_timer_->setSingleShot(true);
    search_timer_->setInterval(search_delay_ms);
    connect(search_, &QLineEdit::textChanged, this, [this](const QString& text) {
      pending_search_ = text;
      search_timer_->start();
    });
    connect(search_timer_, &QTimer::timeout, this, [this] {
      filter_->set_search(pending_search_);
      update_status();
    });
    connect(level_, &QComboBox::currentIndexChanged, this, [this](int) {
      filter_->set_level(level_->currentData().toInt());
      update_status();
    });
    connect(source_, &QComboBox::currentIndexChanged, this, [this](int) {
      filter_->set_source(source_->currentData().toInt());
      update_status();
    });
    connect(target_, &QComboBox::currentIndexChanged, this, [this](int) {
      filter_->set_target(target_->currentData().toString());
      update_status();
    });
    connect(capture_, &QCheckBox::toggled, this, [](bool enabled) {
      set_trace_capture_enabled(enabled);
    });
    connect(
      scheduler_capture_,
      &QCheckBox::toggled,
      this,
      [](bool enabled) { set_scheduler_trace_capture_enabled(enabled); });
    connect(follow_, &QCheckBox::toggled, this, [this](bool enabled) {
      if (enabled) {
        table_->scrollToBottom();
      }
    });
    connect(clear_, &QPushButton::clicked, this, [this] { clear_records(); });
    connect(
      table_->selectionModel(),
      &QItemSelectionModel::currentRowChanged,
      this,
      [this](const QModelIndex& current) { show_details(current); });
    connect(
      table_->verticalScrollBar(),
      &QScrollBar::valueChanged,
      this,
      [this](int value) {
        if (!appending_ && follow_->isChecked()
            && value < table_->verticalScrollBar()->maximum()) {
          follow_->setChecked(false);
        }
      });
    connect(
      table_,
      &QTableView::customContextMenuRequested,
      this,
      [this](const QPoint& position) { show_table_menu(position); });
    connect(
      details_,
      &QTreeWidget::customContextMenuRequested,
      this,
      [this](const QPoint& position) { show_details_menu(position); });

    auto* copy_action = new QAction(
      translate(copy_row_text),
      table_);
    copy_action->setShortcut(QKeySequence::Copy);
    copy_action->setShortcutContext(Qt::WidgetShortcut);
    connect(copy_action, &QAction::triggered, this, [this] { copy_selected_row(); });
    table_->addAction(copy_action);
  }

  void drain_records()
  {
    const auto stats = trace_stats();
    if (stats.session != trace_session_) {
      reset_view_for_session(stats.session);
    }

    auto incoming = drain_trace_records(drain_batch_size);
    if (incoming.empty()) {
      update_status();
      return;
    }

    std::vector<TraceRecord> records;
    records.reserve(incoming.size());
    for (const auto& input : incoming) {
      auto record = make_record(input);
      add_target(record.target);
      records.push_back(std::move(record));
    }

    appending_ = true;
    local_dropped_ += model_->append(std::move(records));
    if (follow_->isChecked()) {
      table_->scrollToBottom();
    }
    appending_ = false;
    update_empty_state();
    update_status();
  }

  void add_target(const QString& target)
  {
    if (target.isEmpty() || known_targets_.contains(target)) {
      return;
    }
    known_targets_.push_back(target);
    std::sort(known_targets_.begin(), known_targets_.end());

    const auto selected = target_->currentData().toString();
    target_->blockSignals(true);
    target_->clear();
    target_->addItem(
      translate(all_targets_text),
      QString());
    for (const auto& known_target : known_targets_) {
      target_->addItem(known_target, known_target);
    }
    const auto index = target_->findData(selected);
    target_->setCurrentIndex(index >= 0 ? index : 0);
    target_->blockSignals(false);
  }

  void clear_records()
  {
    clear_trace_records();
    clear_view(false);
    update_status();
  }

  void reset_view_for_session(std::uint64_t session)
  {
    trace_session_ = session;
    clear_view(true);
    update_status();
  }

  void clear_view(bool reset_evictions)
  {
    model_->clear();
    details_->clear();
    if (reset_evictions) {
      local_dropped_ = 0;
    }
    known_targets_.clear();
    target_->blockSignals(true);
    target_->clear();
    target_->addItem(
      translate(all_targets_text),
      QString());
    target_->blockSignals(false);
    filter_->set_target({});
    update_empty_state();
  }

  void show_details(const QModelIndex& proxy_index)
  {
    details_->clear();
    if (!proxy_index.isValid()) {
      return;
    }
    const auto source_index = filter_->mapToSource(proxy_index);
    const auto* record = model_->record_at(source_index.row());
    if (record == nullptr) {
      return;
    }
    for (const auto& field : record->fields) {
      details_->addTopLevelItem(
        new QTreeWidgetItem({ field.key, field.type, field.value }));
    }
    details_->resizeColumnToContents(0);
    details_->resizeColumnToContents(1);
  }

  QString selected_row_text() const
  {
    const auto index = table_->currentIndex();
    if (!index.isValid()) {
      return {};
    }
    QStringList cells;
    for (auto column = 0; column < TraceTableModel::ColumnCount; ++column) {
      cells.push_back(filter_->index(index.row(), column).data().toString());
    }
    return cells.join(QLatin1Char('\t'));
  }

  void copy_selected_row() const
  {
    const auto text = selected_row_text();
    if (!text.isEmpty()) {
      QApplication::clipboard()->setText(text);
    }
  }

  void show_table_menu(const QPoint& position)
  {
    QMenu menu(table_);
    auto* copy = menu.addAction(
      translate(copy_row_text));
    copy->setEnabled(table_->currentIndex().isValid());
    if (menu.exec(table_->viewport()->mapToGlobal(position)) == copy) {
      copy_selected_row();
    }
  }

  void show_details_menu(const QPoint& position)
  {
    const auto* item = details_->itemAt(position);
    QMenu menu(details_);
    auto* copy = menu.addAction(
      translate(copy_field_text));
    copy->setEnabled(item != nullptr);
    if (menu.exec(details_->viewport()->mapToGlobal(position)) == copy
        && item != nullptr) {
      QApplication::clipboard()->setText(
        QStringLiteral("%1\t%2\t%3")
          .arg(item->text(0), item->text(1), item->text(2)));
    }
  }

  void update_empty_state()
  {
    table_stack_->setCurrentWidget(
      model_->rowCount() == 0 ? static_cast<QWidget*>(empty_)
                              : static_cast<QWidget*>(table_));
  }

  void update_status()
  {
    const auto stats = trace_stats();
    status_->setText(
      translate(status_text)
        .arg(filter_->rowCount())
        .arg(stats.captured)
        .arg(stats.dropped + local_dropped_));
  }

  TraceTableModel* model_ = nullptr;
  TraceFilterModel* filter_ = nullptr;
  QHBoxLayout* controls_ = nullptr;
  QLineEdit* search_ = nullptr;
  QComboBox* level_ = nullptr;
  QComboBox* source_ = nullptr;
  QComboBox* target_ = nullptr;
  QCheckBox* capture_ = nullptr;
  QCheckBox* scheduler_capture_ = nullptr;
  QCheckBox* follow_ = nullptr;
  QPushButton* clear_ = nullptr;
  QTableView* table_ = nullptr;
  QLabel* empty_ = nullptr;
  QStackedWidget* table_stack_ = nullptr;
  QTreeWidget* details_ = nullptr;
  QSplitter* splitter_ = nullptr;
  QLabel* status_ = nullptr;
  QTimer* search_timer_ = nullptr;
  QString pending_search_;
  QStringList known_targets_;
  std::uint64_t trace_session_ = 0;
  std::uint64_t local_dropped_ = 0;
  bool appending_ = false;
};

}

QDockWidget* create_tracing_dock(QMainWindow* parent)
{
  auto* dock = new QDockWidget(
    translate(tracing_text),
    parent);
  dock->setObjectName(QStringLiteral("tracingDock"));
  dock->setAllowedAreas(Qt::AllDockWidgetAreas);
  dock->setFeatures(
    QDockWidget::DockWidgetClosable | QDockWidget::DockWidgetMovable
    | QDockWidget::DockWidgetFloatable);
  dock->setWidget(new TracingPanel(dock));
  return dock;
}

}
