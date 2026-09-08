#pragma once

#include <QDialog>
#include <QString>
#include <QVector>

#include <cstdint>

class QComboBox;
class QLineEdit;
class QTableWidget;

namespace se_ui {

struct NetworkConfiguration;
struct UiSession;

struct ForwardSettings {
    QString protocol, host_address, host_port, guest_address, guest_port;
    bool operator==(const ForwardSettings& other) const {
        return protocol == other.protocol && host_address == other.host_address
            && host_port == other.host_port && guest_address == other.guest_address
            && guest_port == other.guest_port;
    }
};

struct NetworkSettings {
    QString subnet, gateway, dns, dhcp_start;
    QVector<ForwardSettings> forwards;
    bool operator==(const NetworkSettings& other) const {
        return subnet == other.subnet && gateway == other.gateway && dns == other.dns
            && dhcp_start == other.dhcp_start && forwards == other.forwards;
    }
};

NetworkSettings from_network_configuration(const NetworkConfiguration& configuration);
NetworkConfiguration to_network_configuration(const NetworkSettings& settings);

struct MachineSettings {
    QString machine_model;
    std::uint8_t memory_bank_a_simm_mib;
    std::uint8_t memory_bank_b_simm_mib;
    std::uint8_t memory_bank_c_simm_mib;
    QString prom_path;
    QString disk_path;
    QString cdrom_path;
    QString graphics_board;
    QString float_backend;
    NetworkSettings network;
};

class SettingsDialog final : public QDialog {
public:
    explicit SettingsDialog(const UiSession& session, const MachineSettings& settings, QWidget* parent = nullptr);

    [[nodiscard]] MachineSettings settings() const;

private:
    void select_prom();
    void select_disk();
    void select_cdrom();
    void add_forward(const ForwardSettings& rule);

    const UiSession& session_;
    QComboBox* machine_combo_;
    QComboBox* memory_bank_a_combo_;
    QComboBox* memory_bank_b_combo_;
    QComboBox* memory_bank_c_combo_;
    QLineEdit* prom_edit_;
    QLineEdit* disk_edit_;
    QLineEdit* cdrom_edit_;
    QComboBox* graphics_board_combo_;
    QComboBox* float_backend_combo_;
    QLineEdit* subnet_edit_;
    QLineEdit* gateway_edit_;
    QLineEdit* dns_edit_;
    QLineEdit* dhcp_start_edit_;
    QTableWidget* forwards_table_;
};

} // namespace se_ui
