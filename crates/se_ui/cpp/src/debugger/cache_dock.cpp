#include "se_ui/debugger/cache_dock.h"

#include "se_ui/src/bridge.rs.h"

#include <QAbstractTableModel>
#include <QApplication>
#include <QCheckBox>
#include <QClipboard>
#include <QFontDatabase>
#include <QHeaderView>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QLabel>
#include <QPushButton>
#include <QTabBar>
#include <QTableView>
#include <QVBoxLayout>
#include <QWidget>

#include <algorithm>
#include <array>
#include <utility>
#include <vector>

namespace se_ui {
namespace {

struct CacheRow {
    std::uint32_t index;
    std::uint32_t page_frame;
    std::uint32_t word;
    bool valid;
};

class CacheModel final : public QAbstractTableModel {
public:
    explicit CacheModel(std::vector<CacheRow> rows, QObject* parent)
        : QAbstractTableModel(parent)
        , rows_(std::move(rows)) {
    }

    int rowCount(const QModelIndex& parent = {}) const override {
        return parent.isValid() ? 0 : static_cast<int>(rows_.size());
    }

    int columnCount(const QModelIndex& parent = {}) const override {
        return parent.isValid() ? 0 : 4;
    }

    QVariant data(const QModelIndex& index, int role) const override {
        if (!index.isValid() || role != Qt::DisplayRole) {
            return {};
        }
        const auto& row = rows_[static_cast<std::size_t>(index.row())];
        switch (index.column()) {
        case 0:
            return row.index;
        case 1:
            return QStringLiteral("0x%1").arg(row.page_frame, 8, 16, QLatin1Char('0'));
        case 2:
            return QStringLiteral("0x%1").arg(row.word, 8, 16, QLatin1Char('0'));
        case 3:
            return row.valid ? 1 : 0;
        default:
            return {};
        }
    }

    QVariant headerData(int section, Qt::Orientation orientation, int role) const override {
        if (role != Qt::DisplayRole) {
            return {};
        }
        if (orientation == Qt::Vertical) {
            return {};
        }
        static constexpr std::array<const char*, 4> HEADERS = {"Index", "Page frame", "Word", "Valid"};
        return QString::fromLatin1(HEADERS[static_cast<std::size_t>(section)]);
    }

private:
    std::vector<CacheRow> rows_;
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

} // namespace

CacheDock::CacheDock(const UiSession& session, QWidget* parent)
    : QDockWidget(QStringLiteral("Cache"), parent)
    , session_(session)
    , tabs_(new QTabBar(this))
    , valid_only_(new QCheckBox(QStringLiteral("Valid only"), this))
    , status_(new QLabel(QStringLiteral("Press Refresh to sample cache state."), this))
    , refresh_button_(new QPushButton(QStringLiteral("Refresh"), this))
    , table_(new CopyTableView(this)) {
    setObjectName(QStringLiteral("CacheDock"));
    tabs_->addTab(QStringLiteral("Instruction"));
    tabs_->addTab(QStringLiteral("Data"));
    valid_only_->setChecked(true);

    auto* controls = new QHBoxLayout;
    controls->addWidget(tabs_);
    controls->addSpacing(8);
    controls->addWidget(valid_only_);
    controls->addStretch();
    controls->addWidget(refresh_button_);

    table_->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    table_->setSelectionMode(QAbstractItemView::ExtendedSelection);
    table_->setSelectionBehavior(QAbstractItemView::SelectItems);
    table_->setHorizontalScrollMode(QAbstractItemView::ScrollPerPixel);
    table_->setVerticalScrollMode(QAbstractItemView::ScrollPerPixel);
    table_->horizontalHeader()->setStretchLastSection(false);
    table_->setSizeAdjustPolicy(QAbstractScrollArea::AdjustIgnored);

    auto* container = new QWidget(this);
    auto* layout = new QVBoxLayout(container);
    layout->setContentsMargins(6, 6, 6, 6);
    layout->addLayout(controls);
    layout->addWidget(status_);
    layout->addWidget(table_);
    setWidget(container);

    connect(refresh_button_, &QPushButton::clicked, this, &CacheDock::refresh);
    connect(tabs_, &QTabBar::currentChanged, this, [this] { clear(); });
    connect(valid_only_, &QCheckBox::toggled, this, [this] { clear(); });
}

void CacheDock::refresh() {
    const bool instruction = tabs_->currentIndex() == 0;
    const auto data = session_.cache(instruction);
    if (!data.success) {
        status_->setText(QStringLiteral("No machine configured."));
        return;
    }

    std::vector<CacheRow> rows;
    rows.reserve(data.entries.size());
    for (const auto& entry : data.entries) {
        if (!valid_only_->isChecked() || entry.valid) {
            rows.push_back({entry.index, entry.page_frame, entry.word, entry.valid});
        }
    }
    auto* model = new CacheModel(std::move(rows), table_);
    auto* previous = table_->model();
    table_->setModel(model);
    delete previous;
    table_->horizontalHeader()->setSectionResizeMode(QHeaderView::ResizeToContents);
    status_->setText(QStringLiteral("Refill: %1 bytes").arg(data.refill_bytes));
}

void CacheDock::clear() {
    auto* previous = table_->model();
    table_->setModel(nullptr);
    delete previous;
    status_->setText(QStringLiteral("Press Refresh to sample cache state."));
}

} // namespace se_ui
