#include "se_ui/debugger/registers_dock.h"

#include "se_ui/src/bridge.rs.h"

#include <QApplication>
#include <QClipboard>
#include <QFontDatabase>
#include <QFormLayout>
#include <QGroupBox>
#include <QHeaderView>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonValue>
#include <QKeyEvent>
#include <QLabel>
#include <QPushButton>
#include <QScrollArea>
#include <QScrollBar>
#include <QStandardItemModel>
#include <QTabBar>
#include <QTableView>
#include <QTabWidget>
#include <QVBoxLayout>
#include <QWidget>

#include <algorithm>
#include <array>
#include <cstring>
#include <limits>
#include <utility>

namespace se_ui {
namespace {

constexpr std::array<const char*, 32> ABI_NAMES = {
    "zero", "at", "v0", "v1", "a0", "a1", "a2", "a3",
    "t0", "t1", "t2", "t3", "t4", "t5", "t6", "t7",
    "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7",
    "t8", "t9", "k0", "k1", "gp", "sp", "fp", "ra",
};

constexpr std::array<std::pair<int, const char*>, 10> CP0_REGISTERS = {{
    {0, "Index"},
    {1, "Random"},
    {2, "EntryLo"},
    {4, "Context"},
    {8, "BadVAddr"},
    {10, "EntryHi"},
    {12, "Status"},
    {13, "Cause"},
    {14, "EPC"},
    {15, "PRId"},
}};

class CopyTableView final : public QTableView {
public:
    using QTableView::QTableView;

protected:
    void keyPressEvent(QKeyEvent* event) override {
        if (!event->matches(QKeySequence::Copy)) {
            QTableView::keyPressEvent(event);
            return;
        }
        auto indexes = selectionModel()->selectedIndexes();
        std::sort(indexes.begin(), indexes.end(), [](const QModelIndex& lhs, const QModelIndex& rhs) {
            return lhs.row() == rhs.row() ? lhs.column() < rhs.column() : lhs.row() < rhs.row();
        });
        QString text;
        int previous_row = -1;
        for (const auto& index : indexes) {
            if (previous_row >= 0) {
                text += index.row() == previous_row ? QLatin1Char('\t') : QLatin1Char('\n');
            }
            text += index.data().toString();
            previous_row = index.row();
        }
        QApplication::clipboard()->setText(text);
    }
};

QString from_rust_string(const rust::String& value) {
    return QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size()));
}

QString hex32(std::uint32_t value) {
    return QStringLiteral("0x%1").arg(value, 8, 16, QLatin1Char('0'));
}

QString single_text(std::uint32_t word) {
    float value = 0.0F;
    std::memcpy(&value, &word, sizeof(value));
    return QString::number(value, 'g', 8);
}

QString double_text(std::uint32_t high, std::uint32_t low) {
    const std::uint64_t bits = (static_cast<std::uint64_t>(high) << 32) | low;
    double value = 0.0;
    std::memcpy(&value, &bits, sizeof(value));
    return QString::number(value, 'g', 12);
}

QJsonValue pending_value(const rust::String& value) {
    const auto text = from_rust_string(value);
    return text == QStringLiteral("none") ? QJsonValue(QJsonValue::Null) : QJsonValue(text);
}

QString formatted_json(const QJsonObject& object) {
    return QString::fromUtf8(QJsonDocument(object).toJson(QJsonDocument::Indented));
}

QString core_json(const RegistersDto& data) {
    if (data.gpr.size() != ABI_NAMES.size()) {
        return {};
    }

    QJsonObject pending;
    pending.insert(QStringLiteral("delay_slot"), pending_value(data.delay_slot));
    pending.insert(QStringLiteral("gpr"), pending_value(data.pending_gpr));
    pending.insert(QStringLiteral("cp0"), pending_value(data.pending_cp0));
    pending.insert(QStringLiteral("cp1"), pending_value(data.pending_cp1));

    QJsonArray registers;
    for (std::size_t index = 0; index < data.gpr.size(); ++index) {
        QJsonObject reg;
        reg.insert(QStringLiteral("index"), static_cast<int>(index));
        reg.insert(QStringLiteral("abi"), QString::fromLatin1(ABI_NAMES[index]));
        reg.insert(QStringLiteral("hex"), hex32(data.gpr[index]));
        reg.insert(
            QStringLiteral("signed"),
            static_cast<qint64>(static_cast<std::int32_t>(data.gpr[index])));
        registers.append(reg);
    }

    QJsonObject root;
    root.insert(QStringLiteral("pc"), hex32(data.pc));
    root.insert(QStringLiteral("hi"), hex32(data.hi));
    root.insert(QStringLiteral("lo"), hex32(data.lo));
    root.insert(QStringLiteral("pending"), pending);
    root.insert(QStringLiteral("gpr"), registers);
    return formatted_json(root);
}

QString cp0_json(const RegistersDto& data) {
    if (data.cp0.size() < 32 || data.cp0_effective.size() < 3) {
        return {};
    }

    QJsonArray registers;
    for (const auto& [index, name] : CP0_REGISTERS) {
        QJsonObject reg;
        reg.insert(QStringLiteral("index"), index);
        reg.insert(QStringLiteral("name"), QString::fromLatin1(name));
        reg.insert(QStringLiteral("value"), hex32(data.cp0[static_cast<std::size_t>(index)]));
        registers.append(reg);
    }

    QJsonObject effective;
    effective.insert(QStringLiteral("coprocessor_usable"), hex32(data.cp0_effective[0]));
    effective.insert(QStringLiteral("interrupt_control"), hex32(data.cp0_effective[1]));
    effective.insert(QStringLiteral("software_interrupts"), hex32(data.cp0_effective[2]));

    QJsonValue pending(QJsonValue::Null);
    if (data.cp0_pending_effective.size() == 3) {
        QJsonObject values;
        values.insert(
            QStringLiteral("coprocessor_usable"), hex32(data.cp0_pending_effective[0]));
        values.insert(
            QStringLiteral("interrupt_control"), hex32(data.cp0_pending_effective[1]));
        values.insert(
            QStringLiteral("software_interrupts"), hex32(data.cp0_pending_effective[2]));
        pending = values;
    }
    QJsonObject execution_visible;
    execution_visible.insert(QStringLiteral("effective"), effective);
    execution_visible.insert(QStringLiteral("pending"), pending);

    const auto status = data.cp0[12];
    QJsonObject status_fields;
    status_fields.insert(
        QStringLiteral("cu"), QStringLiteral("0x%1").arg((status >> 28) & 0xf, 0, 16));
    status_fields.insert(
        QStringLiteral("bev_ts_pe_cm"),
        QStringLiteral("0x%1").arg((status >> 19) & 0xf, 0, 16));
    status_fields.insert(
        QStringLiteral("swc_isc"),
        QStringLiteral("0b%1").arg((status >> 16) & 0x3, 2, 2, QLatin1Char('0')));
    status_fields.insert(
        QStringLiteral("interrupt_mask"),
        QStringLiteral("0x%1").arg((status >> 8) & 0xff, 2, 16, QLatin1Char('0')));
    status_fields.insert(
        QStringLiteral("mode_stack"),
        QStringLiteral("0b%1").arg(status & 0x3f, 6, 2, QLatin1Char('0')));

    const auto cause = data.cp0[13];
    QJsonObject cause_fields;
    cause_fields.insert(QStringLiteral("branch_delay"), ((cause >> 31) & 1) != 0);
    cause_fields.insert(QStringLiteral("coprocessor_error"), static_cast<int>((cause >> 28) & 0x3));
    cause_fields.insert(
        QStringLiteral("interrupt_pending"),
        QStringLiteral("0x%1").arg((cause >> 8) & 0xff, 2, 16, QLatin1Char('0')));
    cause_fields.insert(QStringLiteral("exception_code"), static_cast<int>((cause >> 2) & 0x1f));

    const auto index = data.cp0[0];
    QJsonObject index_fields;
    index_fields.insert(QStringLiteral("probe_failure"), ((index >> 31) & 1) != 0);
    index_fields.insert(QStringLiteral("index"), static_cast<int>((index >> 8) & 0x3f));

    const auto entry_lo = data.cp0[2];
    QJsonObject entry_lo_fields;
    entry_lo_fields.insert(
        QStringLiteral("page_frame_number"),
        QStringLiteral("0x%1").arg(entry_lo >> 12, 5, 16, QLatin1Char('0')));
    entry_lo_fields.insert(QStringLiteral("noncacheable"), ((entry_lo >> 11) & 1) != 0);
    entry_lo_fields.insert(QStringLiteral("dirty"), ((entry_lo >> 10) & 1) != 0);
    entry_lo_fields.insert(QStringLiteral("valid"), ((entry_lo >> 9) & 1) != 0);
    entry_lo_fields.insert(QStringLiteral("global"), ((entry_lo >> 8) & 1) != 0);

    const auto entry_hi = data.cp0[10];
    QJsonObject entry_hi_fields;
    entry_hi_fields.insert(
        QStringLiteral("virtual_page_number"),
        QStringLiteral("0x%1").arg(entry_hi >> 12, 5, 16, QLatin1Char('0')));
    entry_hi_fields.insert(
        QStringLiteral("asid"),
        QStringLiteral("0x%1").arg((entry_hi >> 6) & 0x3f, 2, 16, QLatin1Char('0')));

    QJsonObject root;
    root.insert(QStringLiteral("register_state"), registers);
    root.insert(QStringLiteral("execution_visible_state"), execution_visible);
    root.insert(QStringLiteral("status_fields"), status_fields);
    root.insert(QStringLiteral("cause_fields"), cause_fields);
    root.insert(QStringLiteral("index_fields"), index_fields);
    root.insert(QStringLiteral("entry_lo_fields"), entry_lo_fields);
    root.insert(QStringLiteral("entry_hi_fields"), entry_hi_fields);
    return formatted_json(root);
}

QString cp1_json(const RegistersDto& data) {
    if (data.cp1.size() != 32) {
        return {};
    }

    QJsonObject register_state;
    register_state.insert(QStringLiteral("fcr0"), hex32(data.fcr0));
    register_state.insert(QStringLiteral("fcr30"), hex32(data.fcr30));
    register_state.insert(QStringLiteral("fcr31"), hex32(data.fcr31));
    register_state.insert(QStringLiteral("float_backend"), from_rust_string(data.float_backend));
    register_state.insert(QStringLiteral("interrupt_output"), data.cp1_interrupt);

    QJsonObject fcr31_fields;
    fcr31_fields.insert(QStringLiteral("condition"), ((data.fcr31 >> 23) & 1) != 0);
    fcr31_fields.insert(QStringLiteral("unimplemented"), ((data.fcr31 >> 17) & 1) != 0);
    fcr31_fields.insert(
        QStringLiteral("cause"), QStringLiteral("0x%1").arg((data.fcr31 >> 12) & 0x1f, 0, 16));
    fcr31_fields.insert(
        QStringLiteral("enable"), QStringLiteral("0x%1").arg((data.fcr31 >> 7) & 0x1f, 0, 16));
    fcr31_fields.insert(
        QStringLiteral("flags"), QStringLiteral("0x%1").arg((data.fcr31 >> 2) & 0x1f, 0, 16));
    fcr31_fields.insert(QStringLiteral("rounding_mode"), static_cast<int>(data.fcr31 & 0x3));

    QJsonArray registers;
    for (std::size_t index = 0; index < data.cp1.size(); ++index) {
        QJsonObject reg;
        reg.insert(QStringLiteral("index"), static_cast<int>(index));
        reg.insert(
            QStringLiteral("name"), QStringLiteral("$f%1").arg(static_cast<qulonglong>(index)));
        reg.insert(QStringLiteral("word"), hex32(data.cp1[index]));
        reg.insert(QStringLiteral("single"), single_text(data.cp1[index]));
        if (index % 2 == 0 && index + 1 < data.cp1.size()) {
            reg.insert(
                QStringLiteral("double_pair"),
                QStringLiteral("$f%1/$f%2")
                    .arg(static_cast<qulonglong>(index))
                    .arg(static_cast<qulonglong>(index + 1)));
            reg.insert(
                QStringLiteral("double"), double_text(data.cp1[index], data.cp1[index + 1]));
        } else {
            reg.insert(QStringLiteral("double_pair"), QJsonValue(QJsonValue::Null));
            reg.insert(QStringLiteral("double"), QJsonValue(QJsonValue::Null));
        }
        registers.append(reg);
    }

    QJsonObject root;
    root.insert(QStringLiteral("register_state"), register_state);
    root.insert(QStringLiteral("fcr31_fields"), fcr31_fields);
    root.insert(QStringLiteral("fgr"), registers);
    return formatted_json(root);
}

QLabel* value_label(QWidget* parent) {
    auto* label = new QLabel(QStringLiteral("-"), parent);
    label->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    label->setTextInteractionFlags(Qt::TextSelectableByMouse | Qt::TextSelectableByKeyboard);
    return label;
}

QGroupBox* value_section(const QString& title, QLabel*& value, QWidget* parent) {
    auto* section = new QGroupBox(title, parent);
    section->setFlat(true);
    value = value_label(section);
    auto* layout = new QVBoxLayout(section);
    layout->setContentsMargins(8, 4, 8, 6);
    layout->addWidget(value);
    return section;
}

template <std::size_t Size>
QGroupBox* form_section(
    const QString& title,
    const std::array<QString, Size>& names,
    std::array<QLabel*, Size>& values,
    QWidget* parent) {
    auto* section = new QGroupBox(title, parent);
    section->setFlat(true);
    auto* layout = new QFormLayout(section);
    layout->setContentsMargins(8, 4, 8, 6);
    layout->setFieldGrowthPolicy(QFormLayout::AllNonFixedFieldsGrow);
    layout->setRowWrapPolicy(QFormLayout::WrapLongRows);
    for (std::size_t index = 0; index < Size; ++index) {
        values[index] = value_label(section);
        layout->addRow(names[index], values[index]);
    }
    return section;
}

QScrollArea* page(QWidget* content, QWidget* parent) {
    auto* scroll = new QScrollArea(parent);
    scroll->setWidgetResizable(true);
    scroll->setFrameShape(QFrame::NoFrame);
    scroll->setWidget(content);
    return scroll;
}

QTableView* table(QWidget* parent) {
    auto* view = new CopyTableView(parent);
    view->setAlternatingRowColors(true);
    view->setSelectionMode(QAbstractItemView::ExtendedSelection);
    view->setSelectionBehavior(QAbstractItemView::SelectItems);
    view->setHorizontalScrollMode(QAbstractItemView::ScrollPerPixel);
    view->setVerticalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    view->setSizeAdjustPolicy(QAbstractScrollArea::AdjustIgnored);
    view->verticalHeader()->hide();
    view->horizontalHeader()->setStretchLastSection(true);
    return view;
}

QGroupBox* table_section(const QString& title, QTableView*& view, QWidget* parent) {
    auto* section = new QGroupBox(title, parent);
    section->setFlat(true);
    view = table(section);
    auto* layout = new QVBoxLayout(section);
    layout->setContentsMargins(8, 4, 8, 6);
    layout->addWidget(view);
    return section;
}

QStandardItem* item(const QString& text, bool fixed_font = false) {
    auto* value = new QStandardItem(text);
    value->setEditable(false);
    if (fixed_font) {
        value->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    }
    return value;
}

void replace_model(QTableView* table, QStandardItemModel* model, int row_count) {
    auto* previous = table->model();
    table->setModel(model);
    delete previous;
    table->horizontalHeader()->setSectionResizeMode(QHeaderView::ResizeToContents);
    table->horizontalHeader()->setStretchLastSection(true);
    const auto row_height = table->verticalHeader()->defaultSectionSize();
    const auto header_height = table->horizontalHeader()->sizeHint().height();
    const auto scroll_height = table->horizontalScrollBar()->sizeHint().height();
    table->setFixedHeight(header_height + row_count * row_height + scroll_height + 2);
}

template <std::size_t Size>
void clear_values(std::array<QLabel*, Size>& values) {
    for (auto* value : values) {
        value->setText(QStringLiteral("-"));
    }
}

} // namespace

struct RegistersDockData {
    QTabWidget* tabs;
    QPushButton* copy_button;
    std::array<QString, 3> copy_json;

    QLabel* pc;
    QLabel* hi;
    QLabel* lo;
    std::array<QLabel*, 4> pending;
    QTableView* gpr;

    QTableView* cp0_registers;
    std::array<QLabel*, 6> execution_visible;
    std::array<QLabel*, 5> status;
    std::array<QLabel*, 4> cause;
    std::array<QLabel*, 2> index;
    std::array<QLabel*, 2> entry_lo;
    std::array<QLabel*, 2> entry_hi;

    std::array<QLabel*, 5> cp1_register_state;
    std::array<QLabel*, 6> fcr31;
    QTableView* fgr;
};

namespace {

QWidget* core_page(RegistersDockData& data, QWidget* parent) {
    auto* content = new QWidget(parent);
    auto* layout = new QVBoxLayout(content);
    layout->setContentsMargins(8, 8, 8, 8);
    layout->setSpacing(6);
    layout->addWidget(value_section(QStringLiteral("PC"), data.pc, content));
    layout->addWidget(value_section(QStringLiteral("HI"), data.hi, content));
    layout->addWidget(value_section(QStringLiteral("LO"), data.lo, content));
    layout->addWidget(form_section(
        QStringLiteral("Pending"),
        std::array<QString, 4> {
            QStringLiteral("Delay slot"),
            QStringLiteral("GPR"),
            QStringLiteral("CP0"),
            QStringLiteral("CP1"),
        },
        data.pending,
        content));
    layout->addWidget(table_section(QStringLiteral("Register"), data.gpr, content));
    layout->addStretch();
    return page(content, parent);
}

QWidget* cp0_page(RegistersDockData& data, QWidget* parent) {
    auto* content = new QWidget(parent);
    auto* layout = new QVBoxLayout(content);
    layout->setContentsMargins(8, 8, 8, 8);
    layout->setSpacing(6);
    layout->addWidget(table_section(
        QStringLiteral("Register state"), data.cp0_registers, content));
    layout->addWidget(form_section(
        QStringLiteral("Execution-visible state"),
        std::array<QString, 6> {
            QStringLiteral("Effective CU"),
            QStringLiteral("Effective interrupt"),
            QStringLiteral("Effective software IP"),
            QStringLiteral("Pending CU"),
            QStringLiteral("Pending interrupt"),
            QStringLiteral("Pending software IP"),
        },
        data.execution_visible,
        content));
    layout->addWidget(form_section(
        QStringLiteral("Status fields"),
        std::array<QString, 5> {
            QStringLiteral("CU"),
            QStringLiteral("BEV/TS/PE/CM"),
            QStringLiteral("SwC/IsC"),
            QStringLiteral("IM"),
            QStringLiteral("Mode stack"),
        },
        data.status,
        content));
    layout->addWidget(form_section(
        QStringLiteral("Cause fields"),
        std::array<QString, 4> {
            QStringLiteral("BD"),
            QStringLiteral("CE"),
            QStringLiteral("IP"),
            QStringLiteral("Exception code"),
        },
        data.cause,
        content));
    layout->addWidget(form_section(
        QStringLiteral("Index fields"),
        std::array<QString, 2> {
            QStringLiteral("Probe failure"),
            QStringLiteral("Index"),
        },
        data.index,
        content));
    layout->addWidget(form_section(
        QStringLiteral("EntryLo fields"),
        std::array<QString, 2> {
            QStringLiteral("PFN"),
            QStringLiteral("N/D/V/G"),
        },
        data.entry_lo,
        content));
    layout->addWidget(form_section(
        QStringLiteral("EntryHi fields"),
        std::array<QString, 2> {
            QStringLiteral("VPN"),
            QStringLiteral("ASID"),
        },
        data.entry_hi,
        content));
    layout->addStretch();
    return page(content, parent);
}

QWidget* cp1_page(RegistersDockData& data, QWidget* parent) {
    auto* content = new QWidget(parent);
    auto* layout = new QVBoxLayout(content);
    layout->setContentsMargins(8, 8, 8, 8);
    layout->setSpacing(6);
    layout->addWidget(form_section(
        QStringLiteral("Register state"),
        std::array<QString, 5> {
            QStringLiteral("FCR0 Implementation/Revision"),
            QStringLiteral("FCR30 EIR"),
            QStringLiteral("FCR31 CSR"),
            QStringLiteral("Backend"),
            QStringLiteral("Interrupt output"),
        },
        data.cp1_register_state,
        content));
    layout->addWidget(form_section(
        QStringLiteral("FCR31 fields"),
        std::array<QString, 6> {
            QStringLiteral("Condition"),
            QStringLiteral("Unimplemented"),
            QStringLiteral("Cause"),
            QStringLiteral("Enable"),
            QStringLiteral("Flags"),
            QStringLiteral("Rounding mode"),
        },
        data.fcr31,
        content));
    layout->addWidget(table_section(QStringLiteral("FGR state"), data.fgr, content));
    layout->addStretch();
    return page(content, parent);
}

void update_gpr(QTableView* table, const RegistersDto& data) {
    auto* model = new QStandardItemModel(table);
    model->setHorizontalHeaderLabels({
        QStringLiteral("Register"),
        QStringLiteral("ABI"),
        QStringLiteral("Hex"),
        QStringLiteral("Signed"),
    });
    for (std::size_t index = 0; index < data.gpr.size(); ++index) {
        QList<QStandardItem*> row;
        row << item(QStringLiteral("$%1").arg(static_cast<qulonglong>(index)), true)
            << item(QString::fromLatin1(ABI_NAMES[index]))
            << item(hex32(data.gpr[index]), true)
            << item(QString::number(static_cast<std::int32_t>(data.gpr[index])), true);
        model->appendRow(row);
    }
    replace_model(table, model, static_cast<int>(data.gpr.size()));
}

void update_cp0_registers(QTableView* table, const RegistersDto& data) {
    auto* model = new QStandardItemModel(table);
    model->setHorizontalHeaderLabels({
        QStringLiteral("Register"),
        QStringLiteral("Name"),
        QStringLiteral("Value"),
    });
    if (data.cp0.size() >= 32) {
        for (const auto& [index, name] : CP0_REGISTERS) {
            QList<QStandardItem*> row;
            row << item(QStringLiteral("$%1").arg(index), true)
                << item(QString::fromLatin1(name))
                << item(hex32(data.cp0[static_cast<std::size_t>(index)]), true);
            model->appendRow(row);
        }
    }
    replace_model(table, model, model->rowCount());
}

void update_fgr(QTableView* table, const RegistersDto& data) {
    auto* model = new QStandardItemModel(table);
    model->setHorizontalHeaderLabels({
        QStringLiteral("FGR"),
        QStringLiteral("Word"),
        QStringLiteral("Single"),
        QStringLiteral("Double pair"),
        QStringLiteral("Double"),
    });
    for (std::size_t index = 0; index < data.cp1.size(); ++index) {
        const auto word = data.cp1[index];
        QString double_pair;
        QString double_value;
        if (index % 2 == 0 && index + 1 < data.cp1.size()) {
            double_pair = QStringLiteral("$f%1/$f%2")
                              .arg(static_cast<qulonglong>(index))
                              .arg(static_cast<qulonglong>(index + 1));
            double_value = double_text(word, data.cp1[index + 1]);
        }
        QList<QStandardItem*> row;
        row << item(QStringLiteral("$f%1").arg(static_cast<qulonglong>(index)), true)
            << item(hex32(word), true)
            << item(single_text(word), true)
            << item(double_pair, true)
            << item(double_value, true);
        model->appendRow(row);
    }
    replace_model(table, model, static_cast<int>(data.cp1.size()));
}

} // namespace

RegistersDock::RegistersDock(
    const UiSession& session,
    std::function<void(const QString&)> report_status,
    QWidget* parent)
    : QDockWidget(QStringLiteral("Registers"), parent)
    , session_(session)
    , report_status_(std::move(report_status))
    , data_(new RegistersDockData)
    , revision_(std::numeric_limits<std::uint64_t>::max()) {
    setObjectName(QStringLiteral("RegistersDock"));
    data_->tabs = new QTabWidget(this);
    data_->tabs->setDocumentMode(true);
    data_->tabs->tabBar()->setExpanding(false);
    data_->tabs->addTab(core_page(*data_, data_->tabs), QStringLiteral("Core"));
    data_->tabs->addTab(cp0_page(*data_, data_->tabs), QStringLiteral("CP0"));
    data_->tabs->addTab(cp1_page(*data_, data_->tabs), QStringLiteral("CP1"));
    data_->copy_button = new QPushButton(QStringLiteral("Copy"), data_->tabs);
    data_->copy_button->setEnabled(false);
    data_->tabs->setCornerWidget(data_->copy_button, Qt::TopRightCorner);
    connect(data_->copy_button, &QPushButton::clicked, this, [this] {
        copy_page(static_cast<std::size_t>(data_->tabs->currentIndex()));
    });
    connect(data_->tabs, &QTabWidget::currentChanged, this, [this](int page) {
        data_->copy_button->setEnabled(
            page >= 0 && !data_->copy_json[static_cast<std::size_t>(page)].isEmpty());
    });
    setWidget(data_->tabs);
}

RegistersDock::~RegistersDock() {
    delete data_;
}

void RegistersDock::copy_page(std::size_t page) {
    static constexpr std::array<const char*, 3> MESSAGES = {
        "Core registers copied.",
        "CP0 registers copied.",
        "CP1 registers copied.",
    };
    if (page >= data_->copy_json.size() || data_->copy_json[page].isEmpty()) {
        return;
    }

    QApplication::clipboard()->setText(data_->copy_json[page]);
    if (report_status_) {
        report_status_(QString::fromLatin1(MESSAGES[page]));
    }
}

void RegistersDock::refresh() {
    const auto data = session_.registers();
    if (!data.success) {
        clear();
        return;
    }
    if (data.revision == revision_) {
        return;
    }
    revision_ = data.revision;

    data_->pc->setText(hex32(data.pc));
    data_->hi->setText(hex32(data.hi));
    data_->lo->setText(hex32(data.lo));
    data_->pending[0]->setText(from_rust_string(data.delay_slot));
    data_->pending[1]->setText(from_rust_string(data.pending_gpr));
    data_->pending[2]->setText(from_rust_string(data.pending_cp0));
    data_->pending[3]->setText(from_rust_string(data.pending_cp1));
    update_gpr(data_->gpr, data);

    update_cp0_registers(data_->cp0_registers, data);
    if (data.cp0.size() >= 32 && data.cp0_effective.size() >= 3) {
        data_->execution_visible[0]->setText(hex32(data.cp0_effective[0]));
        data_->execution_visible[1]->setText(hex32(data.cp0_effective[1]));
        data_->execution_visible[2]->setText(hex32(data.cp0_effective[2]));
        if (data.cp0_pending_effective.size() == 3) {
            data_->execution_visible[3]->setText(hex32(data.cp0_pending_effective[0]));
            data_->execution_visible[4]->setText(hex32(data.cp0_pending_effective[1]));
            data_->execution_visible[5]->setText(hex32(data.cp0_pending_effective[2]));
        } else {
            data_->execution_visible[3]->setText(QStringLiteral("none"));
            data_->execution_visible[4]->setText(QStringLiteral("none"));
            data_->execution_visible[5]->setText(QStringLiteral("none"));
        }

        const auto status = data.cp0[12];
        data_->status[0]->setText(QStringLiteral("0x%1").arg((status >> 28) & 0xf, 0, 16));
        data_->status[1]->setText(QStringLiteral("0x%1").arg((status >> 19) & 0xf, 0, 16));
        data_->status[2]->setText(
            QStringLiteral("0b%1").arg((status >> 16) & 0x3, 2, 2, QLatin1Char('0')));
        data_->status[3]->setText(
            QStringLiteral("0x%1").arg((status >> 8) & 0xff, 2, 16, QLatin1Char('0')));
        data_->status[4]->setText(
            QStringLiteral("0b%1").arg(status & 0x3f, 6, 2, QLatin1Char('0')));

        const auto cause = data.cp0[13];
        data_->cause[0]->setText(QString::number((cause >> 31) & 1));
        data_->cause[1]->setText(QString::number((cause >> 28) & 0x3));
        data_->cause[2]->setText(
            QStringLiteral("0x%1").arg((cause >> 8) & 0xff, 2, 16, QLatin1Char('0')));
        data_->cause[3]->setText(QString::number((cause >> 2) & 0x1f));

        const auto index = data.cp0[0];
        data_->index[0]->setText(QString::number((index >> 31) & 1));
        data_->index[1]->setText(QString::number((index >> 8) & 0x3f));

        const auto entry_lo = data.cp0[2];
        data_->entry_lo[0]->setText(
            QStringLiteral("0x%1").arg(entry_lo >> 12, 5, 16, QLatin1Char('0')));
        data_->entry_lo[1]->setText(QStringLiteral("%1%2%3%4")
                                        .arg((entry_lo >> 11) & 1)
                                        .arg((entry_lo >> 10) & 1)
                                        .arg((entry_lo >> 9) & 1)
                                        .arg((entry_lo >> 8) & 1));

        const auto entry_hi = data.cp0[10];
        data_->entry_hi[0]->setText(
            QStringLiteral("0x%1").arg(entry_hi >> 12, 5, 16, QLatin1Char('0')));
        data_->entry_hi[1]->setText(
            QStringLiteral("0x%1").arg((entry_hi >> 6) & 0x3f, 2, 16, QLatin1Char('0')));
    }

    data_->cp1_register_state[0]->setText(hex32(data.fcr0));
    data_->cp1_register_state[1]->setText(hex32(data.fcr30));
    data_->cp1_register_state[2]->setText(hex32(data.fcr31));
    data_->cp1_register_state[3]->setText(from_rust_string(data.float_backend));
    data_->cp1_register_state[4]->setText(
        data.cp1_interrupt ? QStringLiteral("true") : QStringLiteral("false"));
    data_->fcr31[0]->setText(QString::number((data.fcr31 >> 23) & 1));
    data_->fcr31[1]->setText(QString::number((data.fcr31 >> 17) & 1));
    data_->fcr31[2]->setText(
        QStringLiteral("0x%1").arg((data.fcr31 >> 12) & 0x1f, 0, 16));
    data_->fcr31[3]->setText(
        QStringLiteral("0x%1").arg((data.fcr31 >> 7) & 0x1f, 0, 16));
    data_->fcr31[4]->setText(
        QStringLiteral("0x%1").arg((data.fcr31 >> 2) & 0x1f, 0, 16));
    data_->fcr31[5]->setText(QString::number(data.fcr31 & 0x3));
    update_fgr(data_->fgr, data);

    data_->copy_json[0] = core_json(data);
    data_->copy_json[1] = cp0_json(data);
    data_->copy_json[2] = cp1_json(data);
    data_->copy_button->setEnabled(
        !data_->copy_json[static_cast<std::size_t>(data_->tabs->currentIndex())].isEmpty());
}

void RegistersDock::clear() {
    revision_ = std::numeric_limits<std::uint64_t>::max();
    for (auto& json : data_->copy_json) {
        json.clear();
    }
    data_->copy_button->setEnabled(false);
    data_->pc->setText(QStringLiteral("-"));
    data_->hi->setText(QStringLiteral("-"));
    data_->lo->setText(QStringLiteral("-"));
    clear_values(data_->pending);
    clear_values(data_->execution_visible);
    clear_values(data_->status);
    clear_values(data_->cause);
    clear_values(data_->index);
    clear_values(data_->entry_lo);
    clear_values(data_->entry_hi);
    clear_values(data_->cp1_register_state);
    clear_values(data_->fcr31);

    auto* gpr_model = new QStandardItemModel(data_->gpr);
    replace_model(data_->gpr, gpr_model, 0);
    auto* cp0_model = new QStandardItemModel(data_->cp0_registers);
    replace_model(data_->cp0_registers, cp0_model, 0);
    auto* fgr_model = new QStandardItemModel(data_->fgr);
    replace_model(data_->fgr, fgr_model, 0);
}

} // namespace se_ui
