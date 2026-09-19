#include "se_ui/settings_dialog.h"
#include "se_ui/src/bridge.rs.h"

#include <QApplication>
#include <QCheckBox>
#include <QCloseEvent>
#include <QComboBox>
#include <QDialogButtonBox>
#include <QFileDialog>
#include <QFont>
#include <QFormLayout>
#include <QGroupBox>
#include <QHash>
#include <QHeaderView>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QMessageBox>
#include <QPushButton>
#include <QScrollArea>
#include <QScrollBar>
#include <QSet>
#include <QShowEvent>
#include <QSizePolicy>
#include <QSpinBox>
#include <QSplitter>
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
        from_rust(configuration.dns), from_rust(configuration.dhcp_start),
        from_rust(configuration.tftp_root), from_rust(configuration.bootfile), {} };
    for (const auto& rule : configuration.forwards) {
        result.forwards.append({from_rust(rule.protocol), from_rust(rule.host_address),
            from_rust(rule.host_port), from_rust(rule.guest_address), from_rust(rule.guest_port)});
    }
    return result;
}

NetworkConfiguration to_network_configuration(const NetworkSettings& settings) {
    NetworkConfiguration result {to_rust(settings.subnet), to_rust(settings.gateway),
        to_rust(settings.dns), to_rust(settings.dhcp_start), to_rust(settings.tftp_root),
        to_rust(settings.bootfile), {}};
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
    , current_presentation_id_()
    , tabs_(new QTabWidget(this))
    , machine_tree_(new QTreeWidget(this))
    , detail_scroll_(new QScrollArea(this))
    , detail_content_(new QWidget)
    , detail_layout_(new QVBoxLayout(detail_content_))
    , detail_empty_label_(nullptr)
    , problems_group_(new QGroupBox(this))
    , problems_tree_(new QTreeWidget(this))
    , preflight_timer_(new QTimer(this))
    , preflight_status_(new QLabel(this))
    , subnet_edit_(new QLineEdit(settings.subnet, this))
    , gateway_edit_(new QLineEdit(settings.gateway, this))
    , dns_edit_(new QLineEdit(settings.dns, this))
    , dhcp_start_edit_(new QLineEdit(settings.dhcp_start, this))
    , tftp_root_edit_(new QLineEdit(settings.tftp_root, this))
    , bootfile_edit_(new QLineEdit(settings.bootfile, this))
    , forwards_table_(new QTableWidget(0, 5, this))
    , network_status_(new QLabel(this))
    , reset_notice_(new QLabel(QStringLiteral("Applying changes resets the emulated machine."), this))
    , apply_status_(new QLabel(this))
    , apply_button_(nullptr)
    , cancel_button_(nullptr)
    , machine_rebuild_pending_(false)
    , preflight_pending_(false)
    , applying_(false)
    , network_revision_(0)
    , network_preflight_current_(false)
    , network_error_() {
    setWindowTitle(QStringLiteral("Settings"));
    setModal(true);
    setAttribute(Qt::WA_DeleteOnClose);

    auto* machine_tab = new QWidget(this);
    auto* machine_layout = new QVBoxLayout(machine_tab);
    auto* machine_splitter = new QSplitter(Qt::Horizontal, machine_tab);
    machine_tree_->setHeaderHidden(true);
    detail_scroll_->setWidgetResizable(true);
    detail_scroll_->setWidget(detail_content_);
    machine_splitter->addWidget(machine_tree_);
    machine_splitter->addWidget(detail_scroll_);
    machine_splitter->setChildrenCollapsible(false);
    machine_splitter->setStretchFactor(0, 3);
    machine_splitter->setStretchFactor(1, 5);
    machine_splitter->setSizes({300, 520});
    machine_layout->addWidget(machine_splitter, 1);
    problems_tree_->setColumnCount(2);
    problems_tree_->setHeaderLabels(
        {QStringLiteral("Problem"), QStringLiteral("Location")});
    problems_tree_->setRootIsDecorated(false);
    problems_tree_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    problems_tree_->header()->setSectionResizeMode(1, QHeaderView::Stretch);
    auto* problems_layout = new QVBoxLayout(problems_group_);
    problems_layout->addWidget(problems_tree_);
    problems_tree_->setSizePolicy(QSizePolicy::Preferred, QSizePolicy::Maximum);
    problems_tree_->setMaximumHeight(
        problems_tree_->fontMetrics().height() * 7
        + problems_tree_->header()->sizeHint().height());
    problems_group_->hide();
    machine_layout->addWidget(problems_group_);
    preflight_status_->setWordWrap(true);
    preflight_status_->hide();
    machine_layout->addWidget(preflight_status_);
    connect(machine_tree_, &QTreeWidget::currentItemChanged, this,
        [this](QTreeWidgetItem* current, QTreeWidgetItem*) {
            if (current != nullptr) {
                show_node_details(current->data(0, Qt::UserRole).toString());
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
        if (preflight_pending_ && preflight_handler_) {
            preflight_handler_(this->settings(), network_revision_);
        }
    });

    auto* network_tab = new QWidget(this);
    auto* network_layout = new QVBoxLayout(network_tab);
    auto* network_form = new QFormLayout;
    network_form->addRow(QStringLiteral("IPv4 subnet (CIDR)"), subnet_edit_);
    network_form->addRow(QStringLiteral("Gateway"), gateway_edit_);
    network_form->addRow(QStringLiteral("DNS proxy"), dns_edit_);
    network_form->addRow(QStringLiteral("DHCP start (16 addresses)"), dhcp_start_edit_);
    auto* tftp_row = new QWidget(this);
    auto* tftp_layout = new QHBoxLayout(tftp_row);
    tftp_layout->setContentsMargins(0, 0, 0, 0);
    auto* tftp_browse = new QToolButton(tftp_row);
    tftp_browse->setText(QStringLiteral("..."));
    tftp_browse->setFocusPolicy(Qt::NoFocus);
    tftp_browse->setToolTip(QStringLiteral("Select the host directory served by TFTP."));
    tftp_layout->addWidget(tftp_root_edit_);
    tftp_layout->addWidget(tftp_browse);
    network_form->addRow(QStringLiteral("TFTP root"), tftp_row);
    bootfile_edit_->setToolTip(
        QStringLiteral("Optional BOOTP file name override, e.g. stand/sa."));
    network_form->addRow(QStringLiteral("BOOTP boot filename"), bootfile_edit_);
    connect(tftp_browse, &QToolButton::clicked, this, &SettingsDialog::browse_tftp_root);
    network_layout->addLayout(network_form);
    network_status_->setWordWrap(true);
    network_status_->hide();
    network_layout->addWidget(network_status_);
    forwards_table_->setHorizontalHeaderLabels({QStringLiteral("Protocol"), QStringLiteral("Host address"),
        QStringLiteral("Host port"), QStringLiteral("Guest address"), QStringLiteral("Guest port")});
    forwards_table_->horizontalHeader()->setSectionResizeMode(QHeaderView::Stretch);
    forwards_table_->setSelectionBehavior(QAbstractItemView::SelectRows);
    for (const auto& rule : settings.forwards) { add_forward(rule); }
    network_layout->addWidget(forwards_table_);
    auto* forward_buttons = new QHBoxLayout;
    auto* add = new QPushButton(QStringLiteral("Add forwarding rule"), this);
    auto* remove = new QPushButton(QStringLiteral("Remove selected"), this);
    add->setAutoDefault(false);
    remove->setAutoDefault(false);
    connect(add, &QPushButton::clicked, this, [this] {
        add_forward({QStringLiteral("tcp"), QStringLiteral("127.0.0.1"), QString(),
            dhcp_start_edit_->text(), QString()});
        handle_network_changed();
    });
    connect(remove, &QPushButton::clicked, this, [this] {
        bool removed = false;
        for (int row = forwards_table_->rowCount() - 1; row >= 0; --row) {
            if (forwards_table_->selectionModel()->isRowSelected(row, QModelIndex())) {
                forwards_table_->removeRow(row);
                removed = true;
            }
        }
        if (removed) { handle_network_changed(); }
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
    apply_button_->setText(QStringLiteral("Apply"));
    apply_button_->setAutoDefault(false);
    apply_button_->setDefault(false);
    cancel_button_->setAutoDefault(false);
    // The button is enabled only for a current, successful network preflight;
    // the background apply repeats the host check and reports through
    // `show_apply_failure`, so the user-interface thread never probes the
    // filesystem itself.
    connect(apply_button_, &QPushButton::clicked, this, [this] {
        if (!apply_button_->isEnabled()) { return; }
        auto apply_handler = apply_handler_;
        apply_handler(this->settings());
    });
    connect(cancel_button_, &QPushButton::clicked, this, &SettingsDialog::reject);
    const auto handle_network_edit = [this] { handle_network_changed(); };
    connect(subnet_edit_, &QLineEdit::textEdited, this, handle_network_edit);
    connect(gateway_edit_, &QLineEdit::textEdited, this, handle_network_edit);
    connect(dns_edit_, &QLineEdit::textEdited, this, handle_network_edit);
    connect(dhcp_start_edit_, &QLineEdit::textEdited, this, handle_network_edit);
    connect(tftp_root_edit_, &QLineEdit::textEdited, this, handle_network_edit);
    connect(bootfile_edit_, &QLineEdit::textEdited, this, handle_network_edit);
    connect(forwards_table_, &QTableWidget::itemChanged, this,
        [this](QTableWidgetItem*) { handle_network_changed(); });
    auto* root = new QVBoxLayout(this);
    root->addWidget(tabs_);
    apply_status_->setWordWrap(true);
    apply_status_->hide();
    root->addWidget(apply_status_);
    auto* actions = new QHBoxLayout;
    reset_notice_->setWordWrap(true);
    actions->addWidget(reset_notice_, 1);
    actions->addWidget(button_box);
    root->addLayout(actions);
    rebuild_machine_view();
    update_network_status();
    update_apply_enabled();
    QTimer::singleShot(0, this, [this] { schedule_preflight(); });
    resize(820, 550);
}

SettingsDialog::~SettingsDialog() = default;

NetworkSettings SettingsDialog::settings() const {
    NetworkSettings network {subnet_edit_->text(), gateway_edit_->text(), dns_edit_->text(),
        dhcp_start_edit_->text(), tftp_root_edit_->text(), bootfile_edit_->text(), {}};
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
        applying ? QStringLiteral("Applying...") : QStringLiteral("Apply"));
    apply_status_->setText(applying ? QStringLiteral("Applying settings...") : QString());
    apply_status_->setVisible(applying);
    update_apply_enabled();
}

void SettingsDialog::apply_preflight_result(
    MachinePreflightDto result, std::uint64_t network_revision, QString network_error) {
    const bool machine_current = result.revision == machine_view_->revision;
    if (machine_current) {
        preflight_result_ = std::make_unique<MachinePreflightDto>(std::move(result));
    }
    network_preflight_current_ = network_revision == network_revision_;
    if (network_preflight_current_) { network_error_ = std::move(network_error); }
    // A stale half must not cancel the check that is already queued for it.
    if (machine_current && network_preflight_current_) {
        preflight_timer_->stop();
        preflight_pending_ = false;
    }
    refresh_diagnostics_presentation();
}

void SettingsDialog::show_apply_failure(const QString& message) {
    set_applying(false);
    apply_status_->setText(QStringLiteral("Could not apply settings: %1").arg(message));
    apply_status_->show();
    schedule_preflight();
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

void SettingsDialog::showEvent(QShowEvent* event) {
    QDialog::showEvent(event);
    apply_button_->setDefault(false);
    QTimer::singleShot(0, this, [this] { apply_button_->setDefault(false); });
}

void SettingsDialog::rebuild_machine_view() {
    const bool initial_tree = machine_tree_->topLevelItemCount() == 0;
    QString selected;
    QString focused_property;
    QSet<QString> expanded;
    if (machine_tree_->currentItem() != nullptr) {
        selected = machine_tree_->currentItem()->data(0, Qt::UserRole).toString();
    }
    const auto* focused_widget = QApplication::focusWidget();
    for (auto it = property_editors_.cbegin(); it != property_editors_.cend(); ++it) {
        if (it.value() == focused_widget) {
            focused_property = it.key();
            break;
        }
    }
    for (QTreeWidgetItemIterator it(machine_tree_); *it != nullptr; ++it) {
        if ((*it)->isExpanded()) { expanded.insert((*it)->data(0, Qt::UserRole).toString()); }
    }
    const auto tree_scroll = machine_tree_->verticalScrollBar()->value();
    const auto detail_scroll = detail_scroll_->verticalScrollBar()->value();

    build_domain_index();
    build_presentation_projection();
    clear_detail_panel();
    rebuild_machine_tree(expanded, initial_tree);

    auto* current = tree_items_.value(selected, nullptr);
    if (current == nullptr && machine_tree_->topLevelItemCount() > 0) {
        current = machine_tree_->topLevelItem(0);
    }
    machine_tree_->setCurrentItem(current);
    const auto restored_selection = current == nullptr
        ? QString()
        : current->data(0, Qt::UserRole).toString();
    if (current != nullptr && current_presentation_id_ != restored_selection) {
        show_node_details(restored_selection);
    }
    refresh_diagnostics_presentation();

    machine_tree_->verticalScrollBar()->setValue(tree_scroll);
    if (!selected.isEmpty() && restored_selection == selected) {
        detail_layout_->activate();
        detail_scroll_->verticalScrollBar()->setValue(detail_scroll);
        QTimer::singleShot(0, this, [this, restored_selection, detail_scroll] {
            if (current_presentation_id_ == restored_selection) {
                detail_scroll_->verticalScrollBar()->setValue(detail_scroll);
            }
        });
        if (auto* editor = property_editors_.value(focused_property, nullptr);
            editor != nullptr) {
            editor->setFocus(Qt::OtherFocusReason);
        }
    }
}

void SettingsDialog::build_domain_index() {
    nodes_by_id_.clear();
    node_parents_.clear();
    node_labels_.clear();
    node_children_.clear();
    property_owners_.clear();
    property_labels_.clear();
    for (const auto& node : machine_view_->nodes) {
        const auto id = from_rust(node.id);
        const auto parent_id = from_rust(node.parent_id);
        nodes_by_id_.insert(id, &node);
        node_parents_.insert(id, parent_id);
        node_labels_.insert(id, from_rust(node.label));
        if (!parent_id.isEmpty()) { node_children_[parent_id].append(id); }
        for (const auto& property : node.properties) {
            const auto property_id = from_rust(property.id);
            property_owners_.insert(property_id, id);
            property_labels_.insert(property_id, from_rust(property.label));
        }
    }
}

void SettingsDialog::build_presentation_projection() {
    presentation_anchor_by_node_.clear();
    attached_device_by_owner_.clear();
    presentation_parent_.clear();
    presentation_labels_.clear();

    for (const auto& node : machine_view_->nodes) {
        const auto id = from_rust(node.id);
        presentation_anchor_by_node_.insert(id, id);
        presentation_parent_.insert(id, from_rust(node.parent_id));
        presentation_labels_.insert(id, from_rust(node.label));
    }
    for (const auto& owner : machine_view_->nodes) {
        const auto owner_id = from_rust(owner.id);
        if (!owner.has_attachment || owner.current_device.empty()) { continue; }
        QVector<QString> device_children;
        for (const auto& child_id : node_children_.value(owner_id)) {
            const auto* child = nodes_by_id_.value(child_id, nullptr);
            if (child != nullptr && child->role == MachineNodeRoleDto::Device) {
                device_children.append(child_id);
            }
        }
        if (device_children.size() != 1) { continue; }
        const auto device_id = device_children.front();
        const auto* device = nodes_by_id_.value(device_id, nullptr);
        if (device == nullptr || device->has_attachment) { continue; }
        attached_device_by_owner_.insert(owner_id, device_id);
    }
}

void SettingsDialog::rebuild_machine_tree(
    const QSet<QString>& expanded, bool initial_tree) {
    tree_items_.clear();
    machine_tree_->clear();
    for (const auto& node : machine_view_->nodes) {
        const auto id = from_rust(node.id);
        const auto parent_id = presentation_parent_.value(id);
        auto* parent = tree_items_.value(parent_id, nullptr);
        auto* item = parent == nullptr ? new QTreeWidgetItem(machine_tree_)
                                       : new QTreeWidgetItem(parent);
        item->setText(0, presentation_labels_.value(id));
        item->setData(0, Qt::UserRole, id);
        tree_items_.insert(id, item);
        item->setExpanded(expanded.contains(id) || (initial_tree && parent == nullptr));
    }
}

void SettingsDialog::clear_detail_panel() {
    property_editors_.clear();
    property_diagnostic_containers_.clear();
    property_diagnostic_layouts_.clear();
    node_section_widgets_.clear();
    node_section_has_content_.clear();
    node_diagnostic_containers_.clear();
    node_diagnostic_layouts_.clear();
    current_presentation_id_.clear();
    detail_empty_label_ = nullptr;
    while (auto* item = detail_layout_->takeAt(0)) {
        delete item->widget();
        delete item;
    }
}

QString SettingsDialog::presentation_anchor(const QString& node_id) const {
    if (node_id.isEmpty()) { return QString(); }
    return presentation_anchor_by_node_.value(node_id, node_id);
}

QString SettingsDialog::presentation_breadcrumb(const QString& presentation_id) const {
    QStringList labels;
    QSet<QString> visited;
    auto current = presentation_id;
    while (!current.isEmpty() && presentation_labels_.contains(current)
        && !visited.contains(current)) {
        visited.insert(current);
        const auto parent = presentation_parent_.value(current);
        if (!parent.isEmpty() || current == presentation_id) {
            labels.prepend(presentation_labels_.value(current));
        }
        current = parent;
    }
    return labels.join(QStringLiteral(" > "));
}

QString SettingsDialog::direct_device_label(const QString& node_id) const {
    QString label;
    int device_count = 0;
    for (const auto& child_id : node_children_.value(node_id)) {
        const auto* child = nodes_by_id_.value(child_id, nullptr);
        if (child != nullptr && child->role == MachineNodeRoleDto::Device) {
            label = from_rust(child->label);
            ++device_count;
        }
    }
    return device_count == 1 ? label : QString();
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
            node_id = presentation_anchor(node_id);
            const int severity = diagnostic.severity == 1 ? 2 : 1;
            QSet<QString> visited;
            while (!node_id.isEmpty() && tree_items_.contains(node_id)
                && !visited.contains(node_id)) {
                visited.insert(node_id);
                node_severity.insert(
                    node_id, std::max(node_severity.value(node_id, 0), severity));
                node_id = presentation_parent_.value(node_id);
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
    update_network_status();
    update_apply_enabled();
}

void SettingsDialog::refresh_inline_diagnostics() {
    for (auto it = node_section_widgets_.cbegin(); it != node_section_widgets_.cend(); ++it) {
        it.value()->setVisible(node_section_has_content_.value(it.key(), false));
    }
    for (auto* layout : node_diagnostic_layouts_) { clear_diagnostic_layout(layout); }
    for (auto* container : node_diagnostic_containers_) { container->hide(); }
    for (auto* layout : property_diagnostic_layouts_) { clear_diagnostic_layout(layout); }
    for (auto* container : property_diagnostic_containers_) { container->hide(); }

    for_each_diagnostic(*machine_view_, preflight_result_.get(),
        [this](const MachineDiagnosticDto& diagnostic) {
            const auto target_id = from_rust(diagnostic.target_id);
            if (diagnostic.target_kind == 1) {
                auto* layout = node_diagnostic_layouts_.value(target_id, nullptr);
                auto* container = node_diagnostic_containers_.value(target_id, nullptr);
                if (layout != nullptr && container != nullptr) {
                    layout->addWidget(diagnostic_row(diagnostic));
                    container->show();
                }
            } else if (diagnostic.target_kind == 2) {
                auto* layout = property_diagnostic_layouts_.value(target_id, nullptr);
                auto* container = property_diagnostic_containers_.value(target_id, nullptr);
                if (layout != nullptr && container != nullptr) {
                    layout->addWidget(diagnostic_row(diagnostic));
                    container->show();
                }
            }
        });

    bool has_visible_section = false;
    for (auto it = node_section_widgets_.cbegin(); it != node_section_widgets_.cend(); ++it) {
        const auto* diagnostics = node_diagnostic_containers_.value(it.key(), nullptr);
        const bool visible = node_section_has_content_.value(it.key(), false)
            || (diagnostics != nullptr && !diagnostics->isHidden());
        it.value()->setVisible(visible);
        has_visible_section = has_visible_section || visible;
    }
    if (detail_empty_label_ != nullptr) {
        detail_empty_label_->setVisible(!has_visible_section);
    }
}

void SettingsDialog::schedule_preflight() {
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

void SettingsDialog::update_network_status() {
    if (preflight_pending_) {
        network_status_->setText(QStringLiteral("Checking network settings..."));
        network_status_->show();
    } else if (network_preflight_current_ && !network_error_.isEmpty()) {
        network_status_->setText(network_error_);
        network_status_->show();
    } else {
        network_status_->clear();
        network_status_->hide();
    }
}

void SettingsDialog::update_apply_enabled() {
    const bool current_preflight = preflight_result_ != nullptr
        && preflight_result_->revision == machine_view_->revision;
    apply_button_->setEnabled(!applying_ && machine_view_->success
        && !has_semantic_error() && !preflight_pending_ && current_preflight
        && preflight_result_->success && !has_host_error() && network_preflight_current_
        && network_error_.isEmpty());
}

void SettingsDialog::handle_network_changed() {
    if (applying_) { return; }
    ++network_revision_;
    network_preflight_current_ = false;
    network_error_.clear();
    clear_apply_failure_status();
    schedule_preflight();
}

void SettingsDialog::browse_tftp_root() {
    const auto directory = QFileDialog::getExistingDirectory(
        this, QStringLiteral("Select TFTP root"), tftp_root_edit_->text());
    if (directory.isEmpty() || directory == tftp_root_edit_->text()) { return; }
    tftp_root_edit_->setText(directory);
    handle_network_changed();
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
    const auto anchor = presentation_anchor(node_id);
    if (anchor.isEmpty() || !presentation_labels_.contains(anchor)) {
        return QStringLiteral("Configuration");
    }

    auto location = presentation_breadcrumb(anchor);
    if (target_kind == 2 && property_labels_.contains(target_id)) {
        if (!location.isEmpty()) { location += QStringLiteral(" > "); }
        location += property_labels_.value(target_id);
    }
    return location.isEmpty() ? QStringLiteral("Configuration") : location;
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
    if (machine_rebuild_pending_) {
        QTimer::singleShot(0, this, [this, target_kind, target_id] {
            navigate_to_diagnostic(target_kind, target_id);
        });
        return;
    }
    const auto domain_node_id = target_kind == 1 ? target_id
        : target_kind == 2              ? property_owners_.value(target_id)
                                         : QString();
    const auto presentation_id = presentation_anchor(domain_node_id);
    auto* item = tree_items_.value(presentation_id, nullptr);
    if (item == nullptr) { return; }

    QSet<QString> visited;
    auto current = presentation_id;
    while (!current.isEmpty() && !visited.contains(current)) {
        visited.insert(current);
        if (auto* ancestor = tree_items_.value(current, nullptr); ancestor != nullptr) {
            ancestor->setExpanded(true);
        }
        current = presentation_parent_.value(current);
    }
    if (machine_tree_->currentItem() != item) {
        machine_tree_->setCurrentItem(item);
    } else if (current_presentation_id_ != presentation_id) {
        show_node_details(presentation_id);
    }
    if (target_kind == 2) {
        if (auto* editor = property_editors_.value(target_id, nullptr); editor != nullptr) {
            editor->setFocus(Qt::OtherFocusReason);
            detail_scroll_->ensureWidgetVisible(editor);
        }
    } else {
        if (auto* section = node_section_widgets_.value(domain_node_id, nullptr);
            section != nullptr) {
            detail_scroll_->ensureWidgetVisible(section);
        }
    }
}

void SettingsDialog::show_node_details(const QString& presentation_id) {
    clear_detail_panel();
    const auto* node = nodes_by_id_.value(presentation_id, nullptr);
    if (node == nullptr) { return; }
    current_presentation_id_ = presentation_id;

    auto* title = new QLabel(presentation_labels_.value(presentation_id), detail_content_);
    auto title_font = title->font();
    title_font.setBold(true);
    if (title_font.pointSizeF() > 0.0) {
        title_font.setPointSizeF(title_font.pointSizeF() + 2.0);
    }
    title->setFont(title_font);
    title->setWordWrap(true);
    detail_layout_->addWidget(title);

    auto* breadcrumb = new QLabel(presentation_breadcrumb(presentation_id), detail_content_);
    breadcrumb->setWordWrap(true);
    detail_layout_->addWidget(breadcrumb);

    add_node_section(presentation_id, false);
    const auto attached_device = attached_device_by_owner_.value(presentation_id);
    if (!attached_device.isEmpty()) { add_node_section(attached_device, true); }

    detail_empty_label_ = new QLabel(
        QStringLiteral("No configurable settings for this component."), detail_content_);
    detail_empty_label_->setWordWrap(true);
    detail_layout_->addWidget(detail_empty_label_);
    detail_layout_->addStretch();
    refresh_inline_diagnostics();
}

void SettingsDialog::add_node_section(const QString& node_id, bool grouped) {
    const auto* node = nodes_by_id_.value(node_id, nullptr);
    if (node == nullptr) { return; }

    QWidget* section = nullptr;
    if (grouped) {
        section = new QGroupBox(
            QStringLiteral("%1 settings").arg(node_labels_.value(node_id)), detail_content_);
    } else {
        section = new QWidget(detail_content_);
    }
    auto* section_layout = new QVBoxLayout(section);
    if (!grouped) { section_layout->setContentsMargins(0, 0, 0, 0); }

    auto* diagnostics = new QWidget(section);
    auto* diagnostic_layout = new QVBoxLayout(diagnostics);
    diagnostic_layout->setContentsMargins(0, 0, 0, 0);
    diagnostics->hide();
    section_layout->addWidget(diagnostics);

    auto* form = new QFormLayout;
    form->setFieldGrowthPolicy(QFormLayout::ExpandingFieldsGrow);
    section_layout->addLayout(form);
    if (node->has_attachment) { add_attachment_editor(form, node_id, *node); }
    add_property_editors(form, *node);

    const bool has_content = node->has_attachment || !node->properties.empty();
    node_section_widgets_.insert(node_id, section);
    node_section_has_content_.insert(node_id, has_content);
    node_diagnostic_containers_.insert(node_id, diagnostics);
    node_diagnostic_layouts_.insert(node_id, diagnostic_layout);
    section->setVisible(has_content);
    detail_layout_->addWidget(section);
}

void SettingsDialog::add_attachment_editor(
    QFormLayout* form, const QString& node_id, const MachineNodeDto& node) {
    auto* combo = new QComboBox(detail_content_);
    if (node.allow_empty) { combo->addItem(QStringLiteral("Empty"), QString()); }
    for (const auto& choice : node.device_choices) {
        combo->addItem(from_rust(choice.label), from_rust(choice.id));
    }
    const auto current_device = from_rust(node.current_device);
    auto selected = combo->findData(current_device);
    if (selected < 0 && !current_device.isEmpty()) {
        auto label = direct_device_label(node_id);
        if (label.isEmpty()) { label = QStringLiteral("Unsupported current device"); }
        combo->addItem(label, current_device);
        selected = combo->count() - 1;
    }
    combo->setCurrentIndex(selected);
    connect(combo, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
        [this, node_id, combo](int index) {
            if (index >= 0) {
                apply_attachment_edit(node_id, combo->itemData(index).toString());
            }
        });
    form->addRow(QStringLiteral("Device"), combo);
}

void SettingsDialog::add_property_editors(QFormLayout* form, const MachineNodeDto& node) {
    for (const auto& property : node.properties) {
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
            form->addRow(QStringLiteral("Current value"),
                new QLabel(display_value(property.value), detail_content_));
        }
        switch (property.editor) {
        case 0: {
                auto* checkbox = new QCheckBox(detail_content_);
                checkbox->setChecked(property.value.bool_value);
                connect(checkbox, &QCheckBox::toggled, this,
                    [this, id](bool checked) { apply_property_edit(id, 0, checked, 0, QString()); });
                form->addRow(label, property_editor(id, checkbox, checkbox));
                break;
        }
        case 1: {
                auto* spin = new QSpinBox(detail_content_);
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
                connect(spin, &QSpinBox::editingFinished, this,
                    [this, id, spin] {
                        apply_property_edit(id, 1, false, spin->value(), QString());
                    });
                form->addRow(label, property_editor(id, spin, spin));
                break;
        }
        case 2:
        case 3: {
                auto* line = new QLineEdit(
                    from_rust(property.value.text_value), detail_content_);
                connect(line, &QLineEdit::editingFinished, this,
                    [this, id, line] { apply_text_edit(id, line->text()); });
                if (property.editor == 3) {
                    auto* row = new QWidget(detail_content_);
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
                    form->addRow(label, property_editor(id, row, line));
                } else {
                    form->addRow(label, property_editor(id, line, line));
                }
                break;
        }
        case 4: {
                auto* combo = new QComboBox(detail_content_);
                int selected = -1;
                for (const auto& choice : property.choices) {
                    const auto index = combo->count();
                    if (same_value(choice.value, property.value)) { selected = index; }
                    combo->addItem(from_rust(choice.label));
                    combo->setItemData(index, choice.value.kind, Qt::UserRole);
                    combo->setItemData(index, choice.value.bool_value, Qt::UserRole + 1);
                    combo->setItemData(index,
                        static_cast<qlonglong>(choice.value.integer_value), Qt::UserRole + 2);
                    combo->setItemData(
                        index, from_rust(choice.value.text_value), Qt::UserRole + 3);
                }
                combo->setCurrentIndex(selected);
                connect(combo, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
                    [this, id, combo](int index) {
                        if (index < 0) { return; }
                        apply_property_edit(id,
                            static_cast<std::uint8_t>(
                                combo->itemData(index, Qt::UserRole).toUInt()),
                            combo->itemData(index, Qt::UserRole + 1).toBool(),
                            combo->itemData(index, Qt::UserRole + 2).toLongLong(),
                            combo->itemData(index, Qt::UserRole + 3).toString());
                    });
                form->addRow(label, property_editor(id, combo, combo));
                break;
        }
        default: break;
        }
    }
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
    accept_machine_view(std::move(next));
}

void SettingsDialog::apply_attachment_edit(const QString& node_id, const QString& device_id) {
    MachineConfigurationEditDto edit {1, to_rust(node_id),
        {0, false, 0, rust::String()}, to_rust(device_id)};
    auto next = session_.apply_machine_edit(edit);
    if (!next.success) {
        QMessageBox::warning(this, QStringLiteral("Machine configuration"), from_rust(next.error));
        return;
    }
    accept_machine_view(std::move(next));
}

void SettingsDialog::accept_machine_view(MachineConfigurationViewDto next) {
    clear_apply_failure_status();
    preflight_timer_->stop();
    preflight_result_.reset();
    preflight_pending_ = false;
    nodes_by_id_.clear();
    machine_view_ = std::make_unique<MachineConfigurationViewDto>(std::move(next));
    apply_button_->setEnabled(false);
    if (machine_rebuild_pending_) { return; }
    machine_rebuild_pending_ = true;
    QTimer::singleShot(0, this, [this] {
        machine_rebuild_pending_ = false;
        rebuild_machine_view();
        schedule_preflight();
    });
}

void SettingsDialog::clear_apply_failure_status() {
    if (applying_) { return; }
    apply_status_->clear();
    apply_status_->hide();
}

void SettingsDialog::add_forward(const ForwardSettings& rule) {
    const auto row = forwards_table_->rowCount();
    forwards_table_->insertRow(row);
    auto* protocol = new QComboBox(forwards_table_);
    protocol->addItem(QStringLiteral("TCP"), QStringLiteral("tcp"));
    protocol->addItem(QStringLiteral("UDP"), QStringLiteral("udp"));
    protocol->setCurrentIndex(rule.protocol == QStringLiteral("udp") ? 1 : 0);
    connect(protocol, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
        [this](int) { handle_network_changed(); });
    forwards_table_->setCellWidget(row, 0, protocol);
    forwards_table_->setItem(row, 1, new QTableWidgetItem(rule.host_address));
    forwards_table_->setItem(row, 2, new QTableWidgetItem(rule.host_port));
    forwards_table_->setItem(row, 3, new QTableWidgetItem(rule.guest_address));
    forwards_table_->setItem(row, 4, new QTableWidgetItem(rule.guest_port));
}

} // namespace se_ui
