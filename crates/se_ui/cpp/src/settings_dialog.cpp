#include "se_ui/settings_dialog.h"
#include "se_ui/src/bridge.rs.h"

#include <QCheckBox>
#include <QCloseEvent>
#include <QComboBox>
#include <QDialogButtonBox>
#include <QFileDialog>
#include <QFormLayout>
#include <QGroupBox>
#include <QHash>
#include <QHeaderView>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QMessageBox>
#include <QPushButton>
#include <QSet>
#include <QSpinBox>
#include <QStyle>
#include <QTableWidget>
#include <QTabWidget>
#include <QToolButton>
#include <QTreeWidget>
#include <QTreeWidgetItemIterator>
#include <QTimer>
#include <QVBoxLayout>

#include <algorithm>
#include <limits>
#include <utility>

namespace se_ui {
namespace {

QString from_rust(const rust::String& value) {
    return QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size()));
}

rust::String to_rust(const QString& value) {
    const auto utf8 = value.toUtf8();
    return rust::String(utf8.constData(), static_cast<std::size_t>(utf8.size()));
}

bool same_value(const MachinePropertyValueDto& a, const MachinePropertyValueDto& b) {
    if (a.kind != b.kind) { return false; }
    switch (a.kind) {
    case 0: return a.bool_value == b.bool_value;
    case 1: return a.integer_value == b.integer_value;
    case 2: return a.text_value == b.text_value;
    default: return false;
    }
}

QString display_value(const MachinePropertyValueDto& value) {
    switch (value.kind) {
    case 0: return value.bool_value ? QStringLiteral("true") : QStringLiteral("false");
    case 1: return QString::number(value.integer_value);
    case 2: return from_rust(value.text_value);
    default: return QStringLiteral("Unknown value type");
    }
}

template <typename Function>
void for_each_diagnostic(const MachineConfigurationViewDto& view,
    const MachinePreflightDto* preflight, Function&& function) {
    for (const auto& diagnostic : view.diagnostics) { function(diagnostic); }
    if (preflight == nullptr || !preflight->success || preflight->revision != view.revision) {
        return;
    }
    for (const auto& diagnostic : preflight->diagnostics) { function(diagnostic); }
}

void clear_diagnostic_layout(QVBoxLayout* layout) {
    if (layout == nullptr) { return; }
    while (auto* item = layout->takeAt(0)) {
        delete item->widget();
        delete item;
    }
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

SettingsDialog::SettingsDialog(const UiSession& session, const NetworkSettings& settings,
    ApplyHandler apply_handler, PreflightHandler preflight_handler, QWidget* parent)
    : QDialog(parent)
    , session_(session)
    , apply_handler_(std::move(apply_handler))
    , preflight_handler_(std::move(preflight_handler))
    , machine_view_(std::make_unique<MachineConfigurationViewDto>(session_.begin_machine_edit()))
    , preflight_result_()
    , current_node_id_()
    , node_diagnostic_container_(nullptr)
    , node_diagnostic_layout_(nullptr)
    , tabs_(new QTabWidget(this))
    , machine_tree_(new QTreeWidget(this))
    , property_form_(new QFormLayout)
    , problems_group_(new QGroupBox(this))
    , problems_tree_(new QTreeWidget(this))
    , preflight_timer_(new QTimer(this))
    , preflight_status_(new QLabel(this))
    , subnet_edit_(new QLineEdit(settings.subnet, this))
    , gateway_edit_(new QLineEdit(settings.gateway, this))
    , dns_edit_(new QLineEdit(settings.dns, this))
    , dhcp_start_edit_(new QLineEdit(settings.dhcp_start, this))
    , forwards_table_(new QTableWidget(0, 5, this))
    , apply_status_(new QLabel(this))
    , apply_button_(nullptr)
    , cancel_button_(nullptr)
    , preflight_pending_(false)
    , applying_(false) {
    setWindowTitle(QStringLiteral("Settings"));
    setModal(true);
    setAttribute(Qt::WA_DeleteOnClose);

    auto* machine_tab = new QWidget(this);
    auto* machine_layout = new QVBoxLayout(machine_tab);
    auto* machine_columns = new QHBoxLayout;
    machine_tree_->setHeaderHidden(true);
    machine_columns->addWidget(machine_tree_, 1);
    auto* property_panel = new QWidget(machine_tab);
    property_panel->setLayout(property_form_);
    machine_columns->addWidget(property_panel, 1);
    machine_layout->addLayout(machine_columns, 1);
    problems_tree_->setColumnCount(2);
    problems_tree_->setHeaderLabels(
        {QStringLiteral("Problem"), QStringLiteral("Location")});
    problems_tree_->setRootIsDecorated(false);
    problems_tree_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    problems_tree_->header()->setSectionResizeMode(1, QHeaderView::Stretch);
    auto* problems_layout = new QVBoxLayout(problems_group_);
    problems_layout->addWidget(problems_tree_);
    problems_group_->hide();
    machine_layout->addWidget(problems_group_);
    preflight_status_->setWordWrap(true);
    preflight_status_->hide();
    machine_layout->addWidget(preflight_status_);
    connect(machine_tree_, &QTreeWidget::currentItemChanged, this,
        [this](QTreeWidgetItem* current, QTreeWidgetItem*) {
            if (current != nullptr) {
                show_node_properties(current->data(0, Qt::UserRole).toString());
            }
        });
    connect(problems_tree_, &QTreeWidget::itemClicked, this,
        [this](QTreeWidgetItem* item, int) {
            navigate_to_diagnostic(
                static_cast<std::uint8_t>(item->data(0, Qt::UserRole).toUInt()),
                item->data(0, Qt::UserRole + 1).toString());
        });
    preflight_timer_->setSingleShot(true);
    preflight_timer_->setInterval(275);
    connect(preflight_timer_, &QTimer::timeout, this, [this] {
        if (preflight_pending_ && preflight_handler_) { preflight_handler_(); }
    });

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
    for (const auto& rule : settings.forwards) { add_forward(rule); }
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

    tabs_->addTab(machine_tab, QStringLiteral("Machine"));
    tabs_->addTab(network_tab, QStringLiteral("Network"));
    auto* button_box = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, this);
    apply_button_ = button_box->button(QDialogButtonBox::Ok);
    cancel_button_ = button_box->button(QDialogButtonBox::Cancel);
    apply_button_->setText(QStringLiteral("Apply & Reset"));
    connect(apply_button_, &QPushButton::clicked, this, [this] {
        if (!apply_button_->isEnabled()) { return; }
        const auto error = session_.validate_network_configuration(to_network_configuration(this->settings()));
        if (!error.empty()) {
            QMessageBox::warning(this, QStringLiteral("Network configuration"), from_rust(error));
            return;
        }
        auto apply_handler = apply_handler_;
        apply_handler(this->settings());
    });
    connect(cancel_button_, &QPushButton::clicked, this, &SettingsDialog::reject);
    auto* root = new QVBoxLayout(this);
    root->addWidget(tabs_);
    apply_status_->setWordWrap(true);
    apply_status_->hide();
    root->addWidget(apply_status_);
    root->addWidget(button_box);
    rebuild_machine_view();
    update_apply_enabled();
    QTimer::singleShot(0, this, [this] { schedule_machine_preflight(); });
    resize(820, 550);
}

SettingsDialog::~SettingsDialog() = default;

NetworkSettings SettingsDialog::settings() const {
    NetworkSettings network {subnet_edit_->text(), gateway_edit_->text(), dns_edit_->text(), dhcp_start_edit_->text(), {}};
    for (int row = 0; row < forwards_table_->rowCount(); ++row) {
        const auto* protocol = qobject_cast<QComboBox*>(forwards_table_->cellWidget(row, 0));
        network.forwards.append({protocol->currentData().toString(), forwards_table_->item(row, 1)->text(),
            forwards_table_->item(row, 2)->text(), forwards_table_->item(row, 3)->text(), forwards_table_->item(row, 4)->text()});
    }
    return network;
}

void SettingsDialog::set_applying(bool applying) {
    applying_ = applying;
    if (applying) { preflight_timer_->stop(); }
    tabs_->setEnabled(!applying);
    cancel_button_->setEnabled(!applying);
    apply_button_->setText(
        applying ? QStringLiteral("Applying...") : QStringLiteral("Apply & Reset"));
    apply_status_->setText(applying ? QStringLiteral("Applying settings...") : QString());
    apply_status_->setVisible(applying);
    update_apply_enabled();
}

void SettingsDialog::apply_preflight_result(MachinePreflightDto result) {
    if (result.revision != machine_view_->revision) { return; }
    preflight_timer_->stop();
    preflight_pending_ = false;
    preflight_result_ = std::make_unique<MachinePreflightDto>(std::move(result));
    refresh_diagnostics_presentation();
}

void SettingsDialog::show_apply_failure(const QString& message) {
    set_applying(false);
    apply_status_->setText(QStringLiteral("Could not apply settings: %1").arg(message));
    apply_status_->show();
    schedule_machine_preflight();
}

void SettingsDialog::finish_apply_success() {
    applying_ = false;
    preflight_timer_->stop();
    QDialog::accept();
}

void SettingsDialog::reject() {
    if (!applying_) {
        QDialog::reject();
    }
}

void SettingsDialog::closeEvent(QCloseEvent* event) {
    if (applying_) {
        event->ignore();
        return;
    }
    QDialog::closeEvent(event);
}

void SettingsDialog::rebuild_machine_view() {
    const bool initial_tree = machine_tree_->topLevelItemCount() == 0;
    QString selected;
    QSet<QString> expanded;
    if (machine_tree_->currentItem() != nullptr) {
        selected = machine_tree_->currentItem()->data(0, Qt::UserRole).toString();
    }
    for (QTreeWidgetItemIterator it(machine_tree_); *it != nullptr; ++it) {
        if ((*it)->isExpanded()) { expanded.insert((*it)->data(0, Qt::UserRole).toString()); }
    }
    nodes_by_id_.clear();
    properties_by_id_.clear();
    node_parents_.clear();
    node_labels_.clear();
    property_owners_.clear();
    property_labels_.clear();
    tree_items_.clear();
    property_editors_.clear();
    property_diagnostic_containers_.clear();
    property_diagnostic_layouts_.clear();
    current_node_id_.clear();
    node_diagnostic_container_ = nullptr;
    node_diagnostic_layout_ = nullptr;
    while (property_form_->rowCount() > 0) { property_form_->removeRow(0); }
    machine_tree_->clear();
    for (const auto& node : machine_view_->nodes) {
        const auto id = from_rust(node.id);
        const auto parent_id = from_rust(node.parent_id);
        nodes_by_id_.insert(id, &node);
        node_parents_.insert(id, parent_id);
        node_labels_.insert(id, from_rust(node.label));
        for (const auto& property : node.properties) {
            const auto property_id = from_rust(property.id);
            properties_by_id_.insert(property_id, &property);
            property_owners_.insert(property_id, id);
            property_labels_.insert(property_id, from_rust(property.label));
        }
        auto* parent = tree_items_.value(parent_id, nullptr);
        auto* item = parent == nullptr ? new QTreeWidgetItem(machine_tree_) : new QTreeWidgetItem(parent);
        item->setText(0, from_rust(node.label));
        item->setData(0, Qt::UserRole, id);
        tree_items_.insert(id, item);
        item->setExpanded(expanded.contains(id) || (initial_tree && parent == nullptr));
    }

    auto* current = tree_items_.value(selected, nullptr);
    if (current == nullptr && machine_tree_->topLevelItemCount() > 0) {
        current = machine_tree_->topLevelItem(0);
    }
    machine_tree_->setCurrentItem(current);
    if (current != nullptr) { show_node_properties(current->data(0, Qt::UserRole).toString()); }
    refresh_diagnostics_presentation();
}

void SettingsDialog::refresh_diagnostics_presentation() {
    QHash<QString, int> node_severity;
    for_each_diagnostic(*machine_view_, preflight_result_.get(),
        [this, &node_severity](const MachineDiagnosticDto& diagnostic) {
            QString node_id;
            const auto target_id = from_rust(diagnostic.target_id);
            if (diagnostic.target_kind == 1) {
                node_id = target_id;
            } else if (diagnostic.target_kind == 2) {
                node_id = property_owners_.value(target_id);
            }
            const int severity = diagnostic.severity == 1 ? 2 : 1;
            QSet<QString> visited;
            while (!node_id.isEmpty() && tree_items_.contains(node_id)
                && !visited.contains(node_id)) {
                visited.insert(node_id);
                node_severity.insert(
                    node_id, std::max(node_severity.value(node_id, 0), severity));
                node_id = node_parents_.value(node_id);
            }
        });
    for (auto it = tree_items_.cbegin(); it != tree_items_.cend(); ++it) {
        const auto severity = node_severity.value(it.key(), 0);
        if (severity == 2) {
            it.value()->setIcon(0, style()->standardIcon(QStyle::SP_MessageBoxCritical));
        } else if (severity == 1) {
            it.value()->setIcon(0, style()->standardIcon(QStyle::SP_MessageBoxWarning));
        } else {
            it.value()->setIcon(0, QIcon());
        }
    }

    problems_tree_->clear();
    int problem_count = 0;
    for_each_diagnostic(*machine_view_, preflight_result_.get(),
        [this, &problem_count](const MachineDiagnosticDto& diagnostic) {
            auto* item = new QTreeWidgetItem(problems_tree_);
            item->setIcon(0, style()->standardIcon(diagnostic.severity == 1
                    ? QStyle::SP_MessageBoxCritical
                    : QStyle::SP_MessageBoxWarning));
            item->setText(0, from_rust(diagnostic.message));
            const auto target_id = from_rust(diagnostic.target_id);
            item->setText(
                1, diagnostic_location(diagnostic.target_kind, target_id));
            item->setData(0, Qt::UserRole, diagnostic.target_kind);
            item->setData(0, Qt::UserRole + 1, target_id);
            ++problem_count;
        });
    problems_group_->setTitle(QStringLiteral("Problems (%1)").arg(problem_count));
    problems_group_->setVisible(problem_count > 0);

    refresh_inline_diagnostics();
    update_preflight_status();
    update_apply_enabled();
}

void SettingsDialog::refresh_inline_diagnostics() {
    clear_diagnostic_layout(node_diagnostic_layout_);
    if (node_diagnostic_container_ != nullptr) { node_diagnostic_container_->hide(); }
    for (auto* layout : property_diagnostic_layouts_) { clear_diagnostic_layout(layout); }
    for (auto* container : property_diagnostic_containers_) { container->hide(); }

    for_each_diagnostic(*machine_view_, preflight_result_.get(),
        [this](const MachineDiagnosticDto& diagnostic) {
            const auto target_id = from_rust(diagnostic.target_id);
            if (diagnostic.target_kind == 1 && target_id == current_node_id_
                && node_diagnostic_layout_ != nullptr) {
                node_diagnostic_layout_->addWidget(diagnostic_row(diagnostic));
                node_diagnostic_container_->show();
            } else if (diagnostic.target_kind == 2) {
                auto* layout = property_diagnostic_layouts_.value(target_id, nullptr);
                auto* container = property_diagnostic_containers_.value(target_id, nullptr);
                if (layout != nullptr && container != nullptr) {
                    layout->addWidget(diagnostic_row(diagnostic));
                    container->show();
                }
            }
        });
}

void SettingsDialog::schedule_machine_preflight() {
    preflight_timer_->stop();
    preflight_result_.reset();
    if (applying_ || !machine_view_->success || has_semantic_error()) {
        preflight_pending_ = false;
    } else {
        preflight_pending_ = true;
        preflight_timer_->start();
    }
    refresh_diagnostics_presentation();
}

void SettingsDialog::update_preflight_status() {
    if (!machine_view_->success) {
        preflight_status_->setText(
            QStringLiteral("Machine configuration is unavailable: %1")
                .arg(from_rust(machine_view_->error)));
        preflight_status_->show();
    } else if (preflight_pending_) {
        preflight_status_->setText(QStringLiteral("Checking host resources..."));
        preflight_status_->show();
    } else if (preflight_result_ != nullptr && !preflight_result_->success) {
        preflight_status_->setText(
            QStringLiteral("Could not check host resources: %1")
                .arg(from_rust(preflight_result_->error)));
        preflight_status_->show();
    } else {
        preflight_status_->clear();
        preflight_status_->hide();
    }
}

void SettingsDialog::update_apply_enabled() {
    const bool current_preflight = preflight_result_ != nullptr
        && preflight_result_->revision == machine_view_->revision;
    apply_button_->setEnabled(!applying_ && machine_view_->success
        && !has_semantic_error() && !preflight_pending_ && current_preflight
        && preflight_result_->success && !has_host_error());
}

bool SettingsDialog::has_semantic_error() const {
    return std::any_of(machine_view_->diagnostics.begin(), machine_view_->diagnostics.end(),
        [](const MachineDiagnosticDto& diagnostic) { return diagnostic.severity == 1; });
}

bool SettingsDialog::has_host_error() const {
    return preflight_result_ != nullptr && preflight_result_->success
        && preflight_result_->revision == machine_view_->revision
        && std::any_of(preflight_result_->diagnostics.begin(),
            preflight_result_->diagnostics.end(),
            [](const MachineDiagnosticDto& diagnostic) {
                return diagnostic.severity == 1;
            });
}

QString SettingsDialog::diagnostic_location(
    std::uint8_t target_kind, const QString& target_id) const {
    QString node_id;
    if (target_kind == 1) {
        node_id = target_id;
    } else if (target_kind == 2) {
        node_id = property_owners_.value(target_id);
    }
    if (node_id.isEmpty() || !node_labels_.contains(node_id)) {
        return QStringLiteral("Configuration");
    }

    QStringList labels;
    QSet<QString> visited;
    auto current = node_id;
    while (!current.isEmpty() && node_labels_.contains(current)
        && !visited.contains(current)) {
        visited.insert(current);
        const auto parent = node_parents_.value(current);
        if (!parent.isEmpty() || current == node_id) {
            labels.prepend(node_labels_.value(current));
        }
        current = parent;
    }
    if (target_kind == 2 && property_labels_.contains(target_id)) {
        labels.append(property_labels_.value(target_id));
    }
    return labels.isEmpty() ? QStringLiteral("Configuration")
                            : labels.join(QStringLiteral(" > "));
}

QWidget* SettingsDialog::diagnostic_row(const MachineDiagnosticDto& diagnostic) {
    auto* row = new QWidget(this);
    auto* layout = new QHBoxLayout(row);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setAlignment(Qt::AlignTop);
    auto* icon = new QLabel(row);
    const auto size = style()->pixelMetric(QStyle::PM_SmallIconSize);
    icon->setPixmap(style()
                        ->standardIcon(diagnostic.severity == 1
                                ? QStyle::SP_MessageBoxCritical
                                : QStyle::SP_MessageBoxWarning)
                        .pixmap(size, size));
    auto* message = new QLabel(from_rust(diagnostic.message), row);
    message->setWordWrap(true);
    layout->addWidget(icon);
    layout->addWidget(message, 1);
    return row;
}

QWidget* SettingsDialog::property_editor(
    const QString& property_id, QWidget* editor, QWidget* focus_widget) {
    auto* container = new QWidget(this);
    auto* layout = new QVBoxLayout(container);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(editor);
    auto* diagnostics = new QWidget(container);
    auto* diagnostic_layout = new QVBoxLayout(diagnostics);
    diagnostic_layout->setContentsMargins(0, 0, 0, 0);
    diagnostics->hide();
    layout->addWidget(diagnostics);
    property_editors_.insert(property_id, focus_widget);
    property_diagnostic_containers_.insert(property_id, diagnostics);
    property_diagnostic_layouts_.insert(property_id, diagnostic_layout);
    return container;
}

void SettingsDialog::navigate_to_diagnostic(
    std::uint8_t target_kind, const QString& target_id) {
    const auto node_id = target_kind == 1 ? target_id
        : target_kind == 2              ? property_owners_.value(target_id)
                                         : QString();
    auto* item = tree_items_.value(node_id, nullptr);
    if (item == nullptr) { return; }

    QSet<QString> visited;
    auto current = node_id;
    while (!current.isEmpty() && !visited.contains(current)) {
        visited.insert(current);
        if (auto* ancestor = tree_items_.value(current, nullptr); ancestor != nullptr) {
            ancestor->setExpanded(true);
        }
        current = node_parents_.value(current);
    }
    machine_tree_->setCurrentItem(item);
    show_node_properties(node_id);
    if (target_kind == 2) {
        if (auto* editor = property_editors_.value(target_id, nullptr); editor != nullptr) {
            editor->setFocus(Qt::OtherFocusReason);
        }
    } else {
        machine_tree_->setFocus(Qt::OtherFocusReason);
    }
}

void SettingsDialog::show_node_properties(const QString& node_id) {
    property_editors_.clear();
    property_diagnostic_containers_.clear();
    property_diagnostic_layouts_.clear();
    current_node_id_.clear();
    node_diagnostic_container_ = nullptr;
    node_diagnostic_layout_ = nullptr;
    while (property_form_->rowCount() > 0) { property_form_->removeRow(0); }
    const auto* node = nodes_by_id_.value(node_id, nullptr);
    if (node == nullptr) { return; }
    current_node_id_ = node_id;
    node_diagnostic_container_ = new QWidget(this);
    node_diagnostic_layout_ = new QVBoxLayout(node_diagnostic_container_);
    node_diagnostic_layout_->setContentsMargins(0, 0, 0, 0);
    node_diagnostic_container_->hide();
    property_form_->addRow(node_diagnostic_container_);
    if (node->has_attachment) {
        auto* combo = new QComboBox(this);
        if (node->allow_empty) { combo->addItem(QStringLiteral("Empty"), QString()); }
        for (const auto& choice : node->device_choices) {
            combo->addItem(from_rust(choice.label), from_rust(choice.id));
        }
        const auto current_device = from_rust(node->current_device);
        const auto selected = combo->findData(current_device);
        if (selected < 0 && !current_device.isEmpty()) {
            property_form_->addRow(QStringLiteral("Current device"),
                new QLabel(current_device, this));
        }
        combo->setCurrentIndex(selected);
        connect(combo, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
            [this, node_id, combo](int index) {
                if (index >= 0) {
                    apply_attachment_edit(node_id, combo->itemData(index).toString());
                }
            });
        property_form_->addRow(QStringLiteral("Device"), combo);
    }
    for (const auto& property : node->properties) {
        const auto id = from_rust(property.id);
        const auto label = from_rust(property.label);
        bool editable_value = true;
        switch (property.editor) {
        case 0: editable_value = property.value.kind == 0; break;
        case 1: editable_value = property.value.kind == 1
                && property.value.integer_value >= property.minimum
                && property.value.integer_value <= property.maximum
                && property.value.integer_value >= std::numeric_limits<int>::min()
                && property.value.integer_value <= std::numeric_limits<int>::max(); break;
        case 2:
        case 3: editable_value = property.value.kind == 2; break;
        case 4:
            editable_value = std::any_of(property.choices.begin(), property.choices.end(),
                [&property](const MachineChoiceDto& choice) {
                    return same_value(choice.value, property.value);
                });
            break;
        default: break;
        }
        if (!editable_value) {
            property_form_->addRow(QStringLiteral("Current value"),
                new QLabel(display_value(property.value), this));
        }
        switch (property.editor) {
        case 0: {
                auto* checkbox = new QCheckBox(this);
                checkbox->setChecked(property.value.bool_value);
                connect(checkbox, &QCheckBox::toggled, this,
                    [this, id](bool checked) { apply_property_edit(id, 0, checked, 0, QString()); });
                property_form_->addRow(label, property_editor(id, checkbox, checkbox));
                break;
        }
        case 1: {
                auto* spin = new QSpinBox(this);
                const auto low = static_cast<int>(std::clamp(property.minimum,
                    static_cast<std::int64_t>(std::numeric_limits<int>::min()),
                    static_cast<std::int64_t>(std::numeric_limits<int>::max())));
                const auto high = static_cast<int>(std::clamp(property.maximum,
                    static_cast<std::int64_t>(std::numeric_limits<int>::min()),
                    static_cast<std::int64_t>(std::numeric_limits<int>::max())));
                spin->setRange(low, high);
                spin->setSingleStep(static_cast<int>(std::clamp(property.step,
                    std::int64_t(1), static_cast<std::int64_t>(std::numeric_limits<int>::max()))));
                spin->setValue(static_cast<int>(std::clamp(property.value.integer_value,
                    static_cast<std::int64_t>(low), static_cast<std::int64_t>(high))));
                spin->setSuffix(from_rust(property.unit));
                connect(spin, QOverload<int>::of(&QSpinBox::valueChanged), this,
                    [this, id](int value) { apply_property_edit(id, 1, false, value, QString()); });
                property_form_->addRow(label, property_editor(id, spin, spin));
                break;
        }
        case 2:
        case 3: {
                auto* line = new QLineEdit(from_rust(property.value.text_value), this);
                connect(line, &QLineEdit::editingFinished, this,
                    [this, id, line] { apply_text_edit(id, line->text()); });
                if (property.editor == 3) {
                    auto* row = new QWidget(this);
                    auto* layout = new QHBoxLayout(row);
                    layout->setContentsMargins(0, 0, 0, 0);
                    auto* browse = new QToolButton(row);
                    browse->setText(QStringLiteral("..."));
                    browse->setFocusPolicy(Qt::NoFocus);
                    layout->addWidget(line);
                    layout->addWidget(browse);
                    const auto directory = property.path_kind == 1;
                    connect(browse, &QToolButton::clicked, this, [this, id, line, directory] {
                        const auto path = directory
                            ? QFileDialog::getExistingDirectory(this, QStringLiteral("Select directory"), line->text())
                            : QFileDialog::getOpenFileName(this, QStringLiteral("Select file"), line->text());
                        if (!path.isEmpty()) { apply_text_edit(id, path); }
                    });
                    property_form_->addRow(label, property_editor(id, row, line));
                } else {
                    property_form_->addRow(label, property_editor(id, line, line));
                }
                break;
        }
        case 4: {
                auto* combo = new QComboBox(this);
                int selected = -1;
                for (const auto& choice : property.choices) {
                    if (same_value(choice.value, property.value)) { selected = combo->count(); }
                    combo->addItem(from_rust(choice.label));
                }
                combo->setCurrentIndex(selected);
                connect(combo, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
                    [this, id](int index) {
                        if (index < 0) { return; }
                        const auto* current_property = properties_by_id_.value(id, nullptr);
                        if (current_property == nullptr
                            || static_cast<std::size_t>(index)
                                >= current_property->choices.size()) {
                            return;
                        }
                        const auto& value =
                            current_property->choices[static_cast<std::size_t>(index)].value;
                        apply_property_edit(id, value.kind, value.bool_value,
                            value.integer_value, from_rust(value.text_value));
                    });
                property_form_->addRow(label, property_editor(id, combo, combo));
                break;
        }
        default: break;
        }
    }
    refresh_inline_diagnostics();
}

void SettingsDialog::apply_text_edit(const QString& property_id, const QString& text) {
    apply_property_edit(property_id, 2, false, 0, text);
}

void SettingsDialog::apply_property_edit(const QString& property_id, std::uint8_t kind,
    bool bool_value, std::int64_t integer_value, const QString& text_value) {
    MachineConfigurationEditDto edit {0, to_rust(property_id),
        {kind, bool_value, integer_value, to_rust(text_value)}, rust::String()};
    auto next = session_.apply_machine_edit(edit);
    if (!next.success) {
        QMessageBox::warning(this, QStringLiteral("Machine configuration"), from_rust(next.error));
        return;
    }
    nodes_by_id_.clear();
    properties_by_id_.clear();
    property_editors_.clear();
    machine_view_ = std::make_unique<MachineConfigurationViewDto>(std::move(next));
    schedule_machine_preflight();
    QTimer::singleShot(0, this, [this] { rebuild_machine_view(); });
}

void SettingsDialog::apply_attachment_edit(const QString& node_id, const QString& device_id) {
    MachineConfigurationEditDto edit {1, to_rust(node_id),
        {0, false, 0, rust::String()}, to_rust(device_id)};
    auto next = session_.apply_machine_edit(edit);
    if (!next.success) {
        QMessageBox::warning(this, QStringLiteral("Machine configuration"), from_rust(next.error));
        return;
    }
    nodes_by_id_.clear();
    properties_by_id_.clear();
    property_editors_.clear();
    machine_view_ = std::make_unique<MachineConfigurationViewDto>(std::move(next));
    schedule_machine_preflight();
    QTimer::singleShot(0, this, [this] { rebuild_machine_view(); });
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

} // namespace se_ui
