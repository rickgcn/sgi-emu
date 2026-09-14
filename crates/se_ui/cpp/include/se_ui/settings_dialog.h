#pragma once

#include <QDialog>
#include <QString>
#include <QVector>

#include <cstdint>
#include <memory>

class QComboBox;
class QFormLayout;
class QLabel;
class QLineEdit;
class QTableWidget;
class QTreeWidget;
class QTreeWidgetItem;

namespace se_ui {

struct NetworkConfiguration;
struct MachineConfigurationViewDto;
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

class SettingsDialog final : public QDialog {
public:
    explicit SettingsDialog(const UiSession& session, const NetworkSettings& settings, QWidget* parent = nullptr);
    ~SettingsDialog() override;

    [[nodiscard]] NetworkSettings settings() const;

private:
    void rebuild_machine_view();
    void show_node_properties(const QString& node_id);
    void apply_text_edit(const QString& property_id, const QString& text);
    void apply_property_edit(const QString& property_id, std::uint8_t kind,
        bool bool_value, std::int64_t integer_value, const QString& text_value);
    void apply_attachment_edit(const QString& node_id, const QString& device_id);
    void add_forward(const ForwardSettings& rule);

    const UiSession& session_;
    std::unique_ptr<MachineConfigurationViewDto> machine_view_;
    QTreeWidget* machine_tree_;
    QFormLayout* property_form_;
    QLabel* diagnostics_;
    QLineEdit* subnet_edit_;
    QLineEdit* gateway_edit_;
    QLineEdit* dns_edit_;
    QLineEdit* dhcp_start_edit_;
    QTableWidget* forwards_table_;
};

} // namespace se_ui
