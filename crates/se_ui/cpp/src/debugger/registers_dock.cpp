#include "se_ui/debugger/registers_dock.h"

#include "se_ui/src/bridge.rs.h"

#include <QApplication>
#include <QClipboard>
#include <QFontDatabase>
#include <QFormLayout>
#include <QGroupBox>
#include <QHeaderView>
#include <QKeyEvent>
#include <QLabel>
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
    static constexpr std::array<std::pair<int, const char*>, 10> REGISTERS = {{
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
    auto* model = new QStandardItemModel(table);
    model->setHorizontalHeaderLabels({
        QStringLiteral("Register"),
        QStringLiteral("Name"),
        QStringLiteral("Value"),
    });
    if (data.cp0.size() >= 32) {
        for (const auto& [index, name] : REGISTERS) {
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
        float single = 0.0F;
        std::memcpy(&single, &word, sizeof(single));
        QString double_pair;
        QString double_value;
        if (index % 2 == 0 && index + 1 < data.cp1.size()) {
            const std::uint64_t bits = (static_cast<std::uint64_t>(word) << 32)
                | data.cp1[index + 1];
            double value = 0.0;
            std::memcpy(&value, &bits, sizeof(value));
            double_pair = QStringLiteral("$f%1/$f%2")
                              .arg(static_cast<qulonglong>(index))
                              .arg(static_cast<qulonglong>(index + 1));
            double_value = QString::number(value, 'g', 12);
        }
        QList<QStandardItem*> row;
        row << item(QStringLiteral("$f%1").arg(static_cast<qulonglong>(index)), true)
            << item(hex32(word), true)
            << item(QString::number(single, 'g', 8), true)
            << item(double_pair, true)
            << item(double_value, true);
        model->appendRow(row);
    }
    replace_model(table, model, static_cast<int>(data.cp1.size()));
}

} // namespace

RegistersDock::RegistersDock(const UiSession& session, QWidget* parent)
    : QDockWidget(QStringLiteral("Registers"), parent)
    , session_(session)
    , data_(new RegistersDockData)
    , revision_(std::numeric_limits<std::uint64_t>::max()) {
    setObjectName(QStringLiteral("RegistersDock"));
    data_->tabs = new QTabWidget(this);
    data_->tabs->setDocumentMode(true);
    data_->tabs->tabBar()->setExpanding(false);
    data_->tabs->addTab(core_page(*data_, data_->tabs), QStringLiteral("Core"));
    data_->tabs->addTab(cp0_page(*data_, data_->tabs), QStringLiteral("CP0"));
    data_->tabs->addTab(cp1_page(*data_, data_->tabs), QStringLiteral("CP1"));
    setWidget(data_->tabs);
}

RegistersDock::~RegistersDock() {
    delete data_;
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
}

void RegistersDock::clear() {
    revision_ = std::numeric_limits<std::uint64_t>::max();
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
