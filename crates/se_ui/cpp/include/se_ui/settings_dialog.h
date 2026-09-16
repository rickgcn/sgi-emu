#pragma once

#include <QDialog>
#include <QHash>
#include <QString>
#include <QVector>

#include <cstdint>
#include <functional>
#include <memory>

class QCloseEvent;
class QComboBox;
class QFormLayout;
class QGroupBox;
class QLabel;
class QLineEdit;
class QPushButton;
class QTableWidget;
class QTabWidget;
class QTimer;
class QTreeWidget;
class QTreeWidgetItem;
class QVBoxLayout;
class QWidget;

namespace se_ui {

struct NetworkConfiguration;
struct MachineDiagnosticDto;
struct MachineNodeDto;
struct MachinePreflightDto;
struct MachinePropertyDto;
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
    using ApplyHandler = std::function<void(NetworkSettings)>;
    using PreflightHandler = std::function<void()>;

    explicit SettingsDialog(const UiSession& session, const NetworkSettings& settings,
        ApplyHandler apply_handler, PreflightHandler preflight_handler,
        QWidget* parent = nullptr);
    ~SettingsDialog() override;

    [[nodiscard]] NetworkSettings settings() const;
    void set_applying(bool applying);
    void apply_preflight_result(MachinePreflightDto result);
    void show_apply_failure(const QString& message);
    void finish_apply_success();

protected:
    void reject() override;
    void closeEvent(QCloseEvent* event) override;

private:
    void rebuild_machine_view();
    void refresh_diagnostics_presentation();
    void refresh_inline_diagnostics();
    void show_node_properties(const QString& node_id);
    void schedule_machine_preflight();
    void update_preflight_status();
    void update_apply_enabled();
    [[nodiscard]] bool has_semantic_error() const;
    [[nodiscard]] bool has_host_error() const;
    [[nodiscard]] QString diagnostic_location(
        std::uint8_t target_kind, const QString& target_id) const;
    QWidget* diagnostic_row(const MachineDiagnosticDto& diagnostic);
    QWidget* property_editor(
        const QString& property_id, QWidget* editor, QWidget* focus_widget);
    void navigate_to_diagnostic(std::uint8_t target_kind, const QString& target_id);
    void apply_text_edit(const QString& property_id, const QString& text);
    void apply_property_edit(const QString& property_id, std::uint8_t kind,
        bool bool_value, std::int64_t integer_value, const QString& text_value);
    void apply_attachment_edit(const QString& node_id, const QString& device_id);
    void add_forward(const ForwardSettings& rule);

    const UiSession& session_;
    ApplyHandler apply_handler_;
    PreflightHandler preflight_handler_;
    std::unique_ptr<MachineConfigurationViewDto> machine_view_;
    std::unique_ptr<MachinePreflightDto> preflight_result_;
    QHash<QString, const MachineNodeDto*> nodes_by_id_;
    QHash<QString, const MachinePropertyDto*> properties_by_id_;
    QHash<QString, QString> node_parents_;
    QHash<QString, QString> node_labels_;
    QHash<QString, QString> property_owners_;
    QHash<QString, QString> property_labels_;
    QHash<QString, QTreeWidgetItem*> tree_items_;
    QHash<QString, QWidget*> property_editors_;
    QHash<QString, QWidget*> property_diagnostic_containers_;
    QHash<QString, QVBoxLayout*> property_diagnostic_layouts_;
    QString current_node_id_;
    QWidget* node_diagnostic_container_;
    QVBoxLayout* node_diagnostic_layout_;
    QTabWidget* tabs_;
    QTreeWidget* machine_tree_;
    QFormLayout* property_form_;
    QGroupBox* problems_group_;
    QTreeWidget* problems_tree_;
    QTimer* preflight_timer_;
    QLabel* preflight_status_;
    QLineEdit* subnet_edit_;
    QLineEdit* gateway_edit_;
    QLineEdit* dns_edit_;
    QLineEdit* dhcp_start_edit_;
    QTableWidget* forwards_table_;
    QLabel* apply_status_;
    QPushButton* apply_button_;
    QPushButton* cancel_button_;
    bool preflight_pending_;
    bool applying_;
};

} // namespace se_ui
