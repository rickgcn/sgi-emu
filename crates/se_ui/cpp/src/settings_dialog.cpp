#include "se_ui/settings_dialog.h"
#include "se_ui/src/bridge.rs.h"

#include <QComboBox>
#include <QDialogButtonBox>
#include <QFileDialog>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QLineEdit>
#include <QMessageBox>
#include <QToolButton>
#include <QVariant>
#include <QWidget>
#include <QHeaderView>
#include <QPushButton>
#include <QTableWidget>
#include <QTabWidget>
#include <QVBoxLayout>

namespace se_ui {
namespace {

QString from_rust(const rust::String& value) {
    return QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size()));
}

rust::String to_rust(const QString& value) {
    const auto utf8 = value.toUtf8();
    return rust::String(utf8.constData(), static_cast<std::size_t>(utf8.size()));
}

void populate_memory_bank(QComboBox* combo, std::uint8_t selected_simm_mib) {
    combo->addItem(QStringLiteral("Not installed"), 0);
    combo->addItem(QStringLiteral("4 x 2 MiB"), 2);
    combo->addItem(QStringLiteral("4 x 4 MiB"), 4);
    combo->addItem(QStringLiteral("4 x 8 MiB"), 8);
    const auto index = combo->findData(static_cast<int>(selected_simm_mib));
    combo->setCurrentIndex(index >= 0 ? index : 0);
}

std::uint8_t selected_simm_mib(const QComboBox* combo) {
    return static_cast<std::uint8_t>(combo->currentData().toUInt());
}

} // namespace

NetworkSettings from_network_configuration(const NetworkConfiguration& configuration) {
    NetworkSettings result { from_rust(configuration.subnet), from_rust(configuration.gateway),
        from_rust(configuration.dns), from_rust(configuration.dhcp_start), {} };
    for (const auto& rule : configuration.forwards) {
        result.forwards.append({from_rust(rule.protocol), from_rust(rule.host_address),
            from_rust(rule.host_port), from_rust(rule.guest_address), from_rust(rule.guest_port)});
    }
    return result;
}

NetworkConfiguration to_network_configuration(const NetworkSettings& settings) {
    NetworkConfiguration result {to_rust(settings.subnet), to_rust(settings.gateway),
        to_rust(settings.dns), to_rust(settings.dhcp_start), {}};
    for (const auto& rule : settings.forwards) {
        result.forwards.push_back({to_rust(rule.protocol), to_rust(rule.host_address),
            to_rust(rule.host_port), to_rust(rule.guest_address), to_rust(rule.guest_port)});
    }
    return result;
}

SettingsDialog::SettingsDialog(const UiSession& session, const MachineSettings& settings, QWidget* parent)
    : QDialog(parent)
    , session_(session)
    , machine_combo_(new QComboBox(this))
    , memory_bank_a_combo_(new QComboBox(this))
    , memory_bank_b_combo_(new QComboBox(this))
    , memory_bank_c_combo_(new QComboBox(this))
    , prom_edit_(new QLineEdit(this))
    , disk_edit_(new QLineEdit(this))
    , cdrom_edit_(new QLineEdit(this))
    , graphics_board_combo_(new QComboBox(this))
    , float_backend_combo_(new QComboBox(this))
    , subnet_edit_(new QLineEdit(settings.network.subnet, this))
    , gateway_edit_(new QLineEdit(settings.network.gateway, this))
    , dns_edit_(new QLineEdit(settings.network.dns, this))
    , dhcp_start_edit_(new QLineEdit(settings.network.dhcp_start, this))
    , forwards_table_(new QTableWidget(0, 5, this)) {
    setWindowTitle(QStringLiteral("Settings"));
    setModal(true);

    machine_combo_->addItem(QStringLiteral("Indigo IP12"), QStringLiteral("indigo-ip12"));
    const auto machine_index = machine_combo_->findData(settings.machine_model);
    machine_combo_->setCurrentIndex(machine_index >= 0 ? machine_index : 0);

    populate_memory_bank(memory_bank_a_combo_, settings.memory_bank_a_simm_mib);
    populate_memory_bank(memory_bank_b_combo_, settings.memory_bank_b_simm_mib);
    populate_memory_bank(memory_bank_c_combo_, settings.memory_bank_c_simm_mib);

    prom_edit_->setText(settings.prom_path);
    disk_edit_->setText(settings.disk_path);
    cdrom_edit_->setText(settings.cdrom_path);

    graphics_board_combo_->addItem(QStringLiteral("LG1"), QStringLiteral("lg1"));
    graphics_board_combo_->addItem(QStringLiteral("None"), QStringLiteral("none"));
    const auto graphics_index = graphics_board_combo_->findData(settings.graphics_board);
    graphics_board_combo_->setCurrentIndex(graphics_index >= 0 ? graphics_index : 0);

    float_backend_combo_->addItem(QStringLiteral("SoftFloat"), QStringLiteral("softfloat"));
    float_backend_combo_->addItem(QStringLiteral("Native"), QStringLiteral("native"));
    const auto backend_index = float_backend_combo_->findData(settings.float_backend);
    float_backend_combo_->setCurrentIndex(backend_index >= 0 ? backend_index : 0);

    auto* browse_button = new QToolButton(this);
    browse_button->setText(QStringLiteral("..."));
    connect(browse_button, &QToolButton::clicked, this, &SettingsDialog::select_prom);

    auto* disk_browse_button = new QToolButton(this);
    disk_browse_button->setText(QStringLiteral("..."));
    connect(disk_browse_button, &QToolButton::clicked, this, &SettingsDialog::select_disk);

    auto* cdrom_browse_button = new QToolButton(this);
    cdrom_browse_button->setText(QStringLiteral("..."));
    connect(cdrom_browse_button, &QToolButton::clicked, this, &SettingsDialog::select_cdrom);

    auto* prom_widget = new QWidget(this);
    auto* prom_layout = new QHBoxLayout(prom_widget);
    prom_layout->setContentsMargins(0, 0, 0, 0);
    prom_layout->addWidget(prom_edit_);
    prom_layout->addWidget(browse_button);

    auto* disk_widget = new QWidget(this);
    auto* disk_layout = new QHBoxLayout(disk_widget);
    disk_layout->setContentsMargins(0, 0, 0, 0);
    disk_layout->addWidget(disk_edit_);
    disk_layout->addWidget(disk_browse_button);

    auto* cdrom_widget = new QWidget(this);
    auto* cdrom_layout = new QHBoxLayout(cdrom_widget);
    cdrom_layout->setContentsMargins(0, 0, 0, 0);
    cdrom_layout->addWidget(cdrom_edit_);
    cdrom_layout->addWidget(cdrom_browse_button);

    auto* button_box = new QDialogButtonBox(
        QDialogButtonBox::Ok | QDialogButtonBox::Cancel, this);
    connect(button_box, &QDialogButtonBox::accepted, this, [this] {
        const auto selected = this->settings();
        if (selected.memory_bank_a_simm_mib == 0 && selected.memory_bank_b_simm_mib == 0
            && selected.memory_bank_c_simm_mib == 0) {
            QMessageBox::warning(
                this,
                QStringLiteral("Memory configuration"),
                QStringLiteral("At least one memory bank must be installed."));
            return;
        }
        const auto error = session_.validate_network_configuration(to_network_configuration(selected.network));
        if (!error.empty()) {
            QMessageBox::warning(this, QStringLiteral("Network configuration"), from_rust(error));
            return;
        }
        accept();
    });
    connect(button_box, &QDialogButtonBox::rejected, this, &QDialog::reject);

    auto* machine_tab = new QWidget(this);
    auto* layout = new QFormLayout(machine_tab);
    layout->addRow(QStringLiteral("Machine"), machine_combo_);
    layout->addRow(QStringLiteral("Memory bank A"), memory_bank_a_combo_);
    layout->addRow(QStringLiteral("Memory bank B"), memory_bank_b_combo_);
    layout->addRow(QStringLiteral("Memory bank C"), memory_bank_c_combo_);
    layout->addRow(QStringLiteral("PROM"), prom_widget);
    layout->addRow(QStringLiteral("Disk image"), disk_widget);
    layout->addRow(QStringLiteral("CD-ROM image"), cdrom_widget);
    layout->addRow(QStringLiteral("Graphics board"), graphics_board_combo_);
    layout->addRow(QStringLiteral("Float backend"), float_backend_combo_);
    auto* network_tab = new QWidget(this);
    auto* network_layout = new QVBoxLayout(network_tab);
    auto* network_form = new QFormLayout;
    network_form->addRow(QStringLiteral("IPv4 subnet (CIDR)"), subnet_edit_);
    network_form->addRow(QStringLiteral("Gateway"), gateway_edit_);
    network_form->addRow(QStringLiteral("DNS proxy"), dns_edit_);
    network_form->addRow(QStringLiteral("DHCP start (16 addresses)"), dhcp_start_edit_);
    network_layout->addLayout(network_form);
    forwards_table_->setHorizontalHeaderLabels({QStringLiteral("Protocol"), QStringLiteral("Host address"),
        QStringLiteral("Host port"), QStringLiteral("Guest address"), QStringLiteral("Guest port")});
    forwards_table_->horizontalHeader()->setSectionResizeMode(QHeaderView::Stretch);
    forwards_table_->setSelectionBehavior(QAbstractItemView::SelectRows);
    for (const auto& rule : settings.network.forwards) { add_forward(rule); }
    network_layout->addWidget(forwards_table_);
    auto* forward_buttons = new QHBoxLayout;
    auto* add = new QPushButton(QStringLiteral("Add forwarding rule"), this);
    auto* remove = new QPushButton(QStringLiteral("Remove selected"), this);
    connect(add, &QPushButton::clicked, this, [this] {
        add_forward({QStringLiteral("tcp"), QStringLiteral("127.0.0.1"), QString(),
            dhcp_start_edit_->text(), QString()});
    });
    connect(remove, &QPushButton::clicked, this, [this] {
        for (int row = forwards_table_->rowCount() - 1; row >= 0; --row) {
            if (forwards_table_->selectionModel()->isRowSelected(row, QModelIndex())) {
                forwards_table_->removeRow(row);
            }
        }
    });
    forward_buttons->addWidget(add);
    forward_buttons->addWidget(remove);
    forward_buttons->addStretch();
    network_layout->addLayout(forward_buttons);
    auto* tabs = new QTabWidget(this);
    tabs->addTab(machine_tab, QStringLiteral("Machine"));
    tabs->addTab(network_tab, QStringLiteral("Network"));
    auto* root = new QVBoxLayout(this);
    root->addWidget(tabs);
    root->addWidget(button_box);
    resize(720, 440);
}

MachineSettings SettingsDialog::settings() const {
    NetworkSettings network {subnet_edit_->text(), gateway_edit_->text(), dns_edit_->text(), dhcp_start_edit_->text(), {}};
    for (int row = 0; row < forwards_table_->rowCount(); ++row) {
        const auto* protocol = qobject_cast<QComboBox*>(forwards_table_->cellWidget(row, 0));
        network.forwards.append({protocol->currentData().toString(), forwards_table_->item(row, 1)->text(),
            forwards_table_->item(row, 2)->text(), forwards_table_->item(row, 3)->text(), forwards_table_->item(row, 4)->text()});
    }
    return {
        machine_combo_->currentData().toString(),
        selected_simm_mib(memory_bank_a_combo_),
        selected_simm_mib(memory_bank_b_combo_),
        selected_simm_mib(memory_bank_c_combo_),
        prom_edit_->text(),
        disk_edit_->text(),
        cdrom_edit_->text(),
        graphics_board_combo_->currentData().toString(),
        float_backend_combo_->currentData().toString(),
        network,
    };
}

void SettingsDialog::add_forward(const ForwardSettings& rule) {
    const auto row = forwards_table_->rowCount();
    forwards_table_->insertRow(row);
    auto* protocol = new QComboBox(forwards_table_);
    protocol->addItem(QStringLiteral("TCP"), QStringLiteral("tcp"));
    protocol->addItem(QStringLiteral("UDP"), QStringLiteral("udp"));
    protocol->setCurrentIndex(rule.protocol == QStringLiteral("udp") ? 1 : 0);
    forwards_table_->setCellWidget(row, 0, protocol);
    forwards_table_->setItem(row, 1, new QTableWidgetItem(rule.host_address));
    forwards_table_->setItem(row, 2, new QTableWidgetItem(rule.host_port));
    forwards_table_->setItem(row, 3, new QTableWidgetItem(rule.guest_address));
    forwards_table_->setItem(row, 4, new QTableWidgetItem(rule.guest_port));
}

void SettingsDialog::select_prom() {
    const auto selected_path = QFileDialog::getOpenFileName(
        this, QStringLiteral("Select PROM"), prom_edit_->text());
    if (!selected_path.isEmpty()) {
        prom_edit_->setText(selected_path);
    }
}

void SettingsDialog::select_disk() {
    const auto selected_path = QFileDialog::getOpenFileName(
        this, QStringLiteral("Select disk image"), disk_edit_->text());
    if (!selected_path.isEmpty()) {
        disk_edit_->setText(selected_path);
    }
}

void SettingsDialog::select_cdrom() {
    const auto selected_path = QFileDialog::getOpenFileName(
        this, QStringLiteral("Select CD-ROM image"), cdrom_edit_->text());
    if (!selected_path.isEmpty()) {
        cdrom_edit_->setText(selected_path);
    }
}

} // namespace se_ui
