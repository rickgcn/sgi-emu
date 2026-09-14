#include "se_ui/settings_dialog.h"
#include "se_ui/src/bridge.rs.h"

#include <QCheckBox>
#include <QComboBox>
#include <QDialogButtonBox>
#include <QFileDialog>
#include <QFormLayout>
#include <QHash>
#include <QHeaderView>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QMessageBox>
#include <QPushButton>
#include <QSet>
#include <QSpinBox>
#include <QTableWidget>
#include <QTabWidget>
#include <QToolButton>
#include <QTreeWidget>
#include <QTreeWidgetItemIterator>
#include <QTimer>
#include <QVBoxLayout>

#include <algorithm>
#include <limits>

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

SettingsDialog::SettingsDialog(const UiSession& session, const NetworkSettings& settings, QWidget* parent)
    : QDialog(parent)
    , session_(session)
    , machine_view_(std::make_unique<MachineConfigurationViewDto>(session_.begin_machine_edit()))
    , machine_tree_(new QTreeWidget(this))
    , property_form_(new QFormLayout)
    , diagnostics_(new QLabel(this))
    , subnet_edit_(new QLineEdit(settings.subnet, this))
    , gateway_edit_(new QLineEdit(settings.gateway, this))
    , dns_edit_(new QLineEdit(settings.dns, this))
    , dhcp_start_edit_(new QLineEdit(settings.dhcp_start, this))
    , forwards_table_(new QTableWidget(0, 5, this)) {
    setWindowTitle(QStringLiteral("Settings"));
    setModal(true);

    auto* machine_tab = new QWidget(this);
    auto* machine_layout = new QVBoxLayout(machine_tab);
    auto* machine_columns = new QHBoxLayout;
    machine_tree_->setHeaderHidden(true);
    machine_columns->addWidget(machine_tree_, 1);
    auto* property_panel = new QWidget(machine_tab);
    property_panel->setLayout(property_form_);
    machine_columns->addWidget(property_panel, 1);
    machine_layout->addLayout(machine_columns, 1);
    diagnostics_->setWordWrap(true);
    diagnostics_->setMinimumHeight(65);
    machine_layout->addWidget(diagnostics_);
    connect(machine_tree_, &QTreeWidget::currentItemChanged, this,
        [this](QTreeWidgetItem* current, QTreeWidgetItem*) {
            if (current != nullptr) {
                show_node_properties(current->data(0, Qt::UserRole).toString());
            }
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

    auto* tabs = new QTabWidget(this);
    tabs->addTab(machine_tab, QStringLiteral("Machine"));
    tabs->addTab(network_tab, QStringLiteral("Network"));
    auto* button_box = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, this);
    connect(button_box, &QDialogButtonBox::accepted, this, [this] {
        if (!machine_view_->success) {
            QMessageBox::warning(this, QStringLiteral("Machine configuration"), from_rust(machine_view_->error));
            return;
        }
        for (const auto& diagnostic : machine_view_->diagnostics) {
            if (diagnostic.severity == 1) {
                QMessageBox::warning(this, QStringLiteral("Machine configuration"), from_rust(diagnostic.message));
                return;
            }
        }
        const auto error = session_.validate_network_configuration(to_network_configuration(this->settings()));
        if (!error.empty()) {
            QMessageBox::warning(this, QStringLiteral("Network configuration"), from_rust(error));
            return;
        }
        accept();
    });
    connect(button_box, &QDialogButtonBox::rejected, this, &QDialog::reject);
    auto* root = new QVBoxLayout(this);
    root->addWidget(tabs);
    root->addWidget(button_box);
    rebuild_machine_view();
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

void SettingsDialog::rebuild_machine_view() {
    QString selected;
    QSet<QString> expanded;
    const bool first_render = machine_tree_->topLevelItemCount() == 0;
    if (machine_tree_->currentItem() != nullptr) {
        selected = machine_tree_->currentItem()->data(0, Qt::UserRole).toString();
    }
    for (QTreeWidgetItemIterator it(machine_tree_); *it != nullptr; ++it) {
        if ((*it)->isExpanded()) { expanded.insert((*it)->data(0, Qt::UserRole).toString()); }
    }
    machine_tree_->clear();
    QHash<QString, QTreeWidgetItem*> items;
    for (const auto& node : machine_view_->nodes) {
        const auto id = from_rust(node.id);
        const auto parent_id = from_rust(node.parent_id);
        auto* parent = items.value(parent_id, nullptr);
        auto* item = parent == nullptr ? new QTreeWidgetItem(machine_tree_) : new QTreeWidgetItem(parent);
        item->setText(0, from_rust(node.label));
        item->setData(0, Qt::UserRole, id);
        items.insert(id, item);
        if (first_render || expanded.contains(id)) { item->setExpanded(true); }
    }
    auto* current = items.value(selected, nullptr);
    if (current == nullptr && machine_tree_->topLevelItemCount() > 0) {
        current = machine_tree_->topLevelItem(0);
    }
    machine_tree_->setCurrentItem(current);
    if (current != nullptr) { show_node_properties(current->data(0, Qt::UserRole).toString()); }

    QStringList messages;
    for (const auto& diagnostic : machine_view_->diagnostics) {
        messages.append(QStringLiteral("%1: %2").arg(
            diagnostic.severity == 1 ? QStringLiteral("Error") : QStringLiteral("Warning"),
            from_rust(diagnostic.message)));
    }
    if (!machine_view_->success) { messages.append(from_rust(machine_view_->error)); }
    diagnostics_->setText(messages.isEmpty() ? QStringLiteral("No configuration issues") : messages.join(QLatin1Char('\n')));
}

void SettingsDialog::show_node_properties(const QString& node_id) {
    while (property_form_->rowCount() > 0) { property_form_->removeRow(0); }
    for (const auto& node : machine_view_->nodes) {
        if (from_rust(node.id) != node_id) { continue; }
        if (node.has_attachment) {
            auto* combo = new QComboBox(this);
            if (node.allow_empty) { combo->addItem(QStringLiteral("Empty"), QString()); }
            for (const auto& choice : node.device_choices) {
                combo->addItem(from_rust(choice.label), from_rust(choice.id));
            }
            const auto current_device = from_rust(node.current_device);
            const auto selected = combo->findData(current_device);
            if (selected < 0 && !current_device.isEmpty()) {
                property_form_->addRow(QStringLiteral("Current device"),
                    new QLabel(current_device, this));
            }
            combo->setCurrentIndex(selected);
            connect(combo, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
                [this, node_id, combo](int index) {
                    if (index >= 0) { apply_attachment_edit(node_id, combo->itemData(index).toString()); }
                });
            property_form_->addRow(QStringLiteral("Device"), combo);
        }
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
                property_form_->addRow(QStringLiteral("Current value"),
                    new QLabel(display_value(property.value), this));
            }
            switch (property.editor) {
            case 0: {
                auto* checkbox = new QCheckBox(this);
                checkbox->setChecked(property.value.bool_value);
                connect(checkbox, &QCheckBox::toggled, this,
                    [this, id](bool checked) { apply_property_edit(id, 0, checked, 0, QString()); });
                property_form_->addRow(label, checkbox);
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
                property_form_->addRow(label, spin);
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
                    property_form_->addRow(label, row);
                } else {
                    property_form_->addRow(label, line);
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
                        for (const auto& current_node : machine_view_->nodes) {
                            for (const auto& current_property : current_node.properties) {
                                if (from_rust(current_property.id) != id
                                    || static_cast<std::size_t>(index) >= current_property.choices.size()) { continue; }
                                const auto& value = current_property.choices[static_cast<std::size_t>(index)].value;
                                apply_property_edit(id, value.kind, value.bool_value,
                                    value.integer_value, from_rust(value.text_value));
                                return;
                            }
                        }
                    });
                property_form_->addRow(label, combo);
                break;
            }
            default: break;
            }
        }
        return;
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
    machine_view_ = std::make_unique<MachineConfigurationViewDto>(std::move(next));
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
    machine_view_ = std::make_unique<MachineConfigurationViewDto>(std::move(next));
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
