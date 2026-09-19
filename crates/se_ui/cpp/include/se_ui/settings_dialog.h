#pragma once

#include <QDialog>
#include <QHash>
#include <QSet>
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
class QScrollArea;
class QShowEvent;
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
    /// Host directory served by the built-in TFTP server; empty disables it.
    QString tftp_root;
    /// Boot filename advertised in BOOTP replies; empty leaves it unset.
    QString bootfile;
    QVector<ForwardSettings> forwards;
    bool operator==(const NetworkSettings& other) const {
        return subnet == other.subnet && gateway == other.gateway && dns == other.dns
            && dhcp_start == other.dhcp_start && tftp_root == other.tftp_root
            && bootfile == other.bootfile && forwards == other.forwards;
    }
};

NetworkSettings from_network_configuration(const NetworkConfiguration& configuration);
NetworkConfiguration to_network_configuration(const NetworkSettings& settings);

class SettingsDialog final : public QDialog {
public:
    using ApplyHandler = std::function<void(NetworkSettings)>;
    /// Requests one background check of the supplied network snapshot, which the
    /// handler identifies with the revision it returns the result for.
    using PreflightHandler = std::function<void(NetworkSettings, std::uint64_t)>;

    explicit SettingsDialog(const UiSession& session, const NetworkSettings& settings,
        ApplyHandler apply_handler, PreflightHandler preflight_handler,
        QWidget* parent = nullptr);
    ~SettingsDialog() override;

    [[nodiscard]] NetworkSettings settings() const;
    void set_applying(bool applying);
    /// Applies one background result to the machine and network snapshots it checked.
    void apply_preflight_result(
        MachinePreflightDto result, std::uint64_t network_revision, QString network_error);
    void show_apply_failure(const QString& message);
    void finish_apply_success();

protected:
    void reject() override;
    void closeEvent(QCloseEvent* event) override;
    void showEvent(QShowEvent* event) override;

private:
    void rebuild_machine_view();
    void build_domain_index();
    void build_presentation_projection();
    void rebuild_machine_tree(const QSet<QString>& expanded, bool initial_tree);
    void clear_detail_panel();
    void refresh_diagnostics_presentation();
    void refresh_inline_diagnostics();
    void show_node_details(const QString& presentation_id);
    void add_node_section(const QString& node_id, bool grouped);
    void add_attachment_editor(QFormLayout* form, const QString& node_id,
        const MachineNodeDto& node);
    void add_property_editors(QFormLayout* form, const MachineNodeDto& node);
    void schedule_preflight();
    void update_preflight_status();
    void update_network_status();
    void update_apply_enabled();
    void accept_machine_view(MachineConfigurationViewDto next);
    void clear_apply_failure_status();
    [[nodiscard]] bool has_semantic_error() const;
    [[nodiscard]] bool has_host_error() const;
    [[nodiscard]] QString presentation_anchor(const QString& node_id) const;
    [[nodiscard]] QString presentation_breadcrumb(const QString& presentation_id) const;
    [[nodiscard]] QString direct_device_label(const QString& node_id) const;
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
    void handle_network_changed();
    void browse_tftp_root();

    const UiSession& session_;
    ApplyHandler apply_handler_;
    PreflightHandler preflight_handler_;
    std::unique_ptr<MachineConfigurationViewDto> machine_view_;
    std::unique_ptr<MachinePreflightDto> preflight_result_;
    QHash<QString, const MachineNodeDto*> nodes_by_id_;
    QHash<QString, QString> node_parents_;
    QHash<QString, QString> node_labels_;
    QHash<QString, QVector<QString>> node_children_;
    QHash<QString, QString> property_owners_;
    QHash<QString, QString> property_labels_;
    QHash<QString, QString> presentation_anchor_by_node_;
    QHash<QString, QString> attached_device_by_owner_;
    QHash<QString, QString> presentation_parent_;
    QHash<QString, QString> presentation_labels_;
    QHash<QString, QTreeWidgetItem*> tree_items_;
    QHash<QString, QWidget*> property_editors_;
    QHash<QString, QWidget*> property_diagnostic_containers_;
    QHash<QString, QVBoxLayout*> property_diagnostic_layouts_;
    QHash<QString, QWidget*> node_section_widgets_;
    QHash<QString, bool> node_section_has_content_;
    QHash<QString, QWidget*> node_diagnostic_containers_;
    QHash<QString, QVBoxLayout*> node_diagnostic_layouts_;
    QString current_presentation_id_;
    QTabWidget* tabs_;
    QTreeWidget* machine_tree_;
    QScrollArea* detail_scroll_;
    QWidget* detail_content_;
    QVBoxLayout* detail_layout_;
    QLabel* detail_empty_label_;
    QGroupBox* problems_group_;
    QTreeWidget* problems_tree_;
    QTimer* preflight_timer_;
    QLabel* preflight_status_;
    QLineEdit* subnet_edit_;
    QLineEdit* gateway_edit_;
    QLineEdit* dns_edit_;
    QLineEdit* dhcp_start_edit_;
    QLineEdit* tftp_root_edit_;
    QLineEdit* bootfile_edit_;
    QTableWidget* forwards_table_;
    QLabel* network_status_;
    QLabel* reset_notice_;
    QLabel* apply_status_;
    QPushButton* apply_button_;
    QPushButton* cancel_button_;
    bool machine_rebuild_pending_;
    bool preflight_pending_;
    bool applying_;
    std::uint64_t network_revision_;
    bool network_preflight_current_;
    QString network_error_;
};

} // namespace se_ui
