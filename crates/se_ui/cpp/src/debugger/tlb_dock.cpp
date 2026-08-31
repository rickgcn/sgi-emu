#include "se_ui/debugger/tlb_dock.h"

#include "se_ui/src/bridge.rs.h"

#include <QApplication>
#include <QCheckBox>
#include <QClipboard>
#include <QFontDatabase>
#include <QHeaderView>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QLabel>
#include <QStandardItemModel>
#include <QTabBar>
#include <QTableView>
#include <QVBoxLayout>
#include <QWidget>

#include <algorithm>
#include <limits>

namespace se_ui {
namespace {

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

QString hex32(std::uint32_t value) {
    return QStringLiteral("0x%1").arg(value, 8, 16, QLatin1Char('0'));
}

QStandardItem* item(const QString& text) {
    auto* value = new QStandardItem(text);
    value->setEditable(false);
    return value;
}

} // namespace

TlbDock::TlbDock(const UiSession& session, QWidget* parent)
    : QDockWidget(QStringLiteral("TLB"), parent)
    , session_(session)
    , tabs_(new QTabBar(this))
    , valid_only_(new QCheckBox(QStringLiteral("Valid only"), this))
    , status_(new QLabel(QStringLiteral("No machine configured."), this))
    , table_(new CopyTableView(this))
    , revision_(std::numeric_limits<std::uint64_t>::max()) {
    setObjectName(QStringLiteral("TlbDock"));
    tabs_->addTab(QStringLiteral("Main"));
    tabs_->addTab(QStringLiteral("Instruction"));

    auto* controls = new QHBoxLayout;
    controls->addWidget(tabs_);
    controls->addSpacing(8);
    controls->addWidget(valid_only_);
    controls->addStretch();

    table_->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    table_->setSelectionMode(QAbstractItemView::ExtendedSelection);
    table_->setSelectionBehavior(QAbstractItemView::SelectItems);
    table_->setHorizontalScrollMode(QAbstractItemView::ScrollPerPixel);
    table_->setVerticalScrollMode(QAbstractItemView::ScrollPerPixel);
    table_->horizontalHeader()->setStretchLastSection(false);
    table_->verticalHeader()->hide();
    table_->setSizeAdjustPolicy(QAbstractScrollArea::AdjustIgnored);

    auto* container = new QWidget(this);
    auto* layout = new QVBoxLayout(container);
    layout->setContentsMargins(6, 6, 6, 6);
    layout->addLayout(controls);
    layout->addWidget(status_);
    layout->addWidget(table_);
    setWidget(container);

    connect(tabs_, &QTabBar::currentChanged, this, [this] {
        revision_ = std::numeric_limits<std::uint64_t>::max();
        refresh();
    });
    connect(valid_only_, &QCheckBox::toggled, this, [this] {
        revision_ = std::numeric_limits<std::uint64_t>::max();
        refresh();
    });
}

void TlbDock::refresh() {
    const bool instruction = tabs_->currentIndex() == 1;
    const auto data = session_.tlb(instruction);
    if (!data.success) {
        clear();
        return;
    }
    if (data.revision == revision_) {
        return;
    }
    revision_ = data.revision;
    status_->setText(QStringLiteral("Shutdown: %1    Index: %2    Random: %3")
                         .arg(data.shutdown ? QStringLiteral("true") : QStringLiteral("false"))
                         .arg(data.index)
                         .arg(data.random));

    auto* model = new QStandardItemModel(table_);
    model->setHorizontalHeaderLabels({
        QStringLiteral("Index"), QStringLiteral("EntryHi"), QStringLiteral("EntryLo"),
        QStringLiteral("VPN"), QStringLiteral("ASID"), QStringLiteral("PFN"),
        QStringLiteral("Cache"), QStringLiteral("D"), QStringLiteral("V"),
        QStringLiteral("G"),
    });
    for (const auto& entry : data.entries) {
        if (valid_only_->isChecked() && !entry.valid) {
            continue;
        }
        QList<QStandardItem*> row;
        row << item(QString::number(entry.index))
            << item(hex32(entry.entry_hi))
            << item(hex32(entry.entry_lo))
            << item(hex32(entry.vpn))
            << item(QStringLiteral("0x%1").arg(entry.asid, 2, 16, QLatin1Char('0')))
            << item(hex32(entry.pfn))
            << item(entry.noncacheable ? QStringLiteral("Uncached") : QStringLiteral("Cached"))
            << item(QString::number(entry.dirty))
            << item(QString::number(entry.valid))
            << item(QString::number(entry.global));
        model->appendRow(row);
    }
    auto* previous = table_->model();
    table_->setModel(model);
    delete previous;
    table_->horizontalHeader()->setSectionResizeMode(QHeaderView::ResizeToContents);
}

void TlbDock::clear() {
    revision_ = std::numeric_limits<std::uint64_t>::max();
    status_->setText(QStringLiteral("No machine configured."));
    auto* previous = table_->model();
    table_->setModel(nullptr);
    delete previous;
}

} // namespace se_ui
