#include "se_ui/settings_dialog.h"

#include <QComboBox>
#include <QDialogButtonBox>
#include <QFileDialog>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QLineEdit>
#include <QToolButton>
#include <QVariant>
#include <QWidget>

namespace se_ui {

SettingsDialog::SettingsDialog(const MachineSettings& settings, QWidget* parent)
    : QDialog(parent)
    , machine_combo_(new QComboBox(this))
    , prom_edit_(new QLineEdit(this))
    , disk_edit_(new QLineEdit(this))
    , cdrom_edit_(new QLineEdit(this))
    , float_backend_combo_(new QComboBox(this)) {
    setWindowTitle(QStringLiteral("Settings"));
    setModal(true);

    machine_combo_->addItem(QStringLiteral("Indigo IP12"), QStringLiteral("indigo-ip12"));
    const auto machine_index = machine_combo_->findData(settings.machine_model);
    machine_combo_->setCurrentIndex(machine_index >= 0 ? machine_index : 0);

    prom_edit_->setText(settings.prom_path);
    disk_edit_->setText(settings.disk_path);
    cdrom_edit_->setText(settings.cdrom_path);

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
    connect(button_box, &QDialogButtonBox::accepted, this, &QDialog::accept);
    connect(button_box, &QDialogButtonBox::rejected, this, &QDialog::reject);

    auto* layout = new QFormLayout(this);
    layout->addRow(QStringLiteral("Machine"), machine_combo_);
    layout->addRow(QStringLiteral("PROM"), prom_widget);
    layout->addRow(QStringLiteral("Disk image"), disk_widget);
    layout->addRow(QStringLiteral("CD-ROM image"), cdrom_widget);
    layout->addRow(QStringLiteral("Float backend"), float_backend_combo_);
    layout->addRow(button_box);
    setLayout(layout);
}

MachineSettings SettingsDialog::settings() const {
    return {
        machine_combo_->currentData().toString(),
        prom_edit_->text(),
        disk_edit_->text(),
        cdrom_edit_->text(),
        float_backend_combo_->currentData().toString(),
    };
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
