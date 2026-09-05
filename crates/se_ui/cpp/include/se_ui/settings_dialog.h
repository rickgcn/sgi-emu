#pragma once

#include <QDialog>
#include <QString>

#include <cstdint>

class QComboBox;
class QLineEdit;

namespace se_ui {

struct MachineSettings {
    QString machine_model;
    std::uint8_t memory_bank_a_simm_mib;
    std::uint8_t memory_bank_b_simm_mib;
    std::uint8_t memory_bank_c_simm_mib;
    QString prom_path;
    QString disk_path;
    QString cdrom_path;
    QString float_backend;
};

class SettingsDialog final : public QDialog {
public:
    explicit SettingsDialog(const MachineSettings& settings, QWidget* parent = nullptr);

    [[nodiscard]] MachineSettings settings() const;

private:
    void select_prom();
    void select_disk();
    void select_cdrom();

    QComboBox* machine_combo_;
    QComboBox* memory_bank_a_combo_;
    QComboBox* memory_bank_b_combo_;
    QComboBox* memory_bank_c_combo_;
    QLineEdit* prom_edit_;
    QLineEdit* disk_edit_;
    QLineEdit* cdrom_edit_;
    QComboBox* float_backend_combo_;
};

} // namespace se_ui
