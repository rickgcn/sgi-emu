use std::collections::{BTreeMap, BTreeSet};

use se_config::definition::MachineDefinition;
use se_config::diagnostic::{Diagnostic, DiagnosticSeverity, DiagnosticTarget};
use se_config::draft::MachineDraft;
use se_config::id::{DeviceKindId, MachineModelId, NodeId, PropertyId};
use se_config::value::PropertyValue;
use se_config::view::{
    AttachmentView, ChoiceOption, ConfigurationView, DeviceChoice, NodeRole, PropertyEditor,
    PropertyView, TopologyNode,
};

pub struct FakeMachine;

pub fn root_id() -> NodeId {
    NodeId("fake-root".into())
}

pub fn cpu_id(index: usize) -> NodeId {
    NodeId(format!("socket:{index}"))
}

pub fn bank_id(index: usize) -> NodeId {
    NodeId(format!("ram@{index}"))
}

pub fn bank_size_id(index: usize) -> PropertyId {
    PropertyId(format!("capacity:{index}"))
}

fn default_bank_size(index: usize) -> i64 {
    if index == 0 { 4 } else { 0 }
}

pub fn controller_id(index: usize) -> NodeId {
    NodeId(format!("controller#{index}"))
}

pub fn slot_id(controller: usize, slot: usize) -> NodeId {
    NodeId(format!("bay:{controller}:{slot}"))
}

pub fn device_id(controller: usize, slot: usize) -> NodeId {
    NodeId(format!("occupant:{controller}:{slot}"))
}

pub fn serial_id(index: usize) -> NodeId {
    NodeId(format!("port@{index}"))
}

pub fn disk_id() -> DeviceKindId {
    DeviceKindId("storage.disk".into())
}

pub fn cdrom_id() -> DeviceKindId {
    DeviceKindId("storage.cdrom".into())
}

fn node(id: NodeId, parent: Option<NodeId>, role: NodeRole, label: String) -> TopologyNode {
    TopologyNode {
        id,
        parent,
        role,
        label,
        properties: Vec::new(),
        attachment: None,
    }
}

fn error(code: &str, target: DiagnosticTarget, message: &str) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Error,
        code: code.into(),
        target,
        message: message.into(),
    }
}

impl MachineDefinition for FakeMachine {
    fn model_id(&self) -> MachineModelId {
        MachineModelId("fake-machine".into())
    }

    fn display_name(&self) -> &str {
        "Fake Machine"
    }

    fn default_draft(&self) -> MachineDraft {
        let properties = (0..5)
            .map(|index| {
                (
                    bank_size_id(index),
                    PropertyValue::Integer(default_bank_size(index)),
                )
            })
            .collect::<BTreeMap<_, _>>();

        MachineDraft {
            model: self.model_id(),
            properties,
            attachments: BTreeMap::new(),
        }
    }

    fn resolve(&self, draft: &MachineDraft) -> ConfigurationView {
        let mut view = ConfigurationView {
            model: self.model_id(),
            display_name: self.display_name().into(),
            nodes: vec![node(root_id(), None, NodeRole::Root, "Fake Machine".into())],
            diagnostics: Vec::new(),
        };

        if draft.model != self.model_id() {
            view.diagnostics.push(error(
                "fake.model.mismatch",
                DiagnosticTarget::Global,
                "The draft belongs to a different machine model.",
            ));
        }

        for index in 0..2 {
            view.nodes.push(node(
                cpu_id(index),
                Some(root_id()),
                NodeRole::Slot,
                format!("CPU Slot {index}"),
            ));
        }

        let mut populated_banks = 0;
        let known_properties = (0..5).map(bank_size_id).collect::<BTreeSet<_>>();

        for index in 0..5 {
            let property = bank_size_id(index);
            let value = match draft.properties.get(&property) {
                Some(value) => value.clone(),
                None => {
                    view.diagnostics.push(error(
                        "fake.property.missing",
                        DiagnosticTarget::Property(property.clone()),
                        "This required property is missing from the draft.",
                    ));
                    PropertyValue::Integer(default_bank_size(index))
                }
            };
            let choices = if index == 4 {
                &[0, 2, 4, 8][..]
            } else {
                &[0, 2, 4, 8, 16][..]
            };

            let valid_size = choices
                .iter()
                .any(|size| value == PropertyValue::Integer(*size));
            if !valid_size {
                view.diagnostics.push(error(
                    "fake.memory.invalid-size",
                    DiagnosticTarget::Property(property.clone()),
                    "This memory bank size is unsupported.",
                ));
            } else if let PropertyValue::Integer(size) = value
                && size > 0
            {
                populated_banks += 1;
            }

            let mut bank = node(
                bank_id(index),
                Some(root_id()),
                NodeRole::Component,
                format!("Memory Bank {index}"),
            );
            bank.properties.push(PropertyView {
                id: property,
                label: "Size".into(),
                value,
                editor: PropertyEditor::Choice {
                    options: choices
                        .iter()
                        .map(|size| ChoiceOption {
                            value: PropertyValue::Integer(*size),
                            label: format!("{size} MiB"),
                        })
                        .collect(),
                },
            });
            view.nodes.push(bank);
        }

        if populated_banks == 0 {
            view.diagnostics.push(error(
                "fake.memory.no-installed-bank",
                DiagnosticTarget::Node(root_id()),
                "At least one memory bank must be populated.",
            ));
        }

        for property in draft.properties.keys() {
            if !known_properties.contains(property) {
                view.diagnostics.push(error(
                    "fake.property.unknown",
                    DiagnosticTarget::Property(property.clone()),
                    "This property is not defined by the machine.",
                ));
            }
        }

        let mut known_slots = BTreeSet::new();
        for (controller, slot_count) in [3, 2].into_iter().enumerate() {
            view.nodes.push(node(
                controller_id(controller),
                Some(root_id()),
                NodeRole::Component,
                format!("Storage Controller {controller}"),
            ));

            for slot in 0..slot_count {
                let slot_identity = slot_id(controller, slot);
                known_slots.insert(slot_identity.clone());
                let current = draft.attachments.get(&slot_identity).cloned();
                let mut choices = vec![DeviceChoice {
                    id: disk_id(),
                    label: "Disk".into(),
                }];
                if slot != 0 {
                    choices.push(DeviceChoice {
                        id: cdrom_id(),
                        label: "CD-ROM".into(),
                    });
                }

                let mut slot_node = node(
                    slot_identity.clone(),
                    Some(controller_id(controller)),
                    NodeRole::Slot,
                    format!("Slot {slot}"),
                );
                slot_node.attachment = Some(AttachmentView {
                    allow_empty: true,
                    current: current.clone(),
                    choices: choices.clone(),
                });
                view.nodes.push(slot_node);

                if let Some(device) = current {
                    let supported = choices.iter().any(|choice| choice.id == device);
                    if !supported {
                        view.diagnostics.push(error(
                            "fake.attachment.unsupported",
                            DiagnosticTarget::Node(slot_identity.clone()),
                            "This device kind is unsupported in this slot.",
                        ));
                    }

                    let label = if device == disk_id() {
                        "Disk".into()
                    } else if device == cdrom_id() {
                        "CD-ROM".into()
                    } else {
                        format!("Unsupported device ({})", device.0)
                    };
                    view.nodes.push(node(
                        device_id(controller, slot),
                        Some(slot_identity),
                        NodeRole::Device,
                        label,
                    ));
                }
            }
        }

        for slot in draft.attachments.keys() {
            if !known_slots.contains(slot) {
                view.diagnostics.push(error(
                    "fake.attachment.unknown-slot",
                    DiagnosticTarget::Node(slot.clone()),
                    "This slot is not defined by the machine.",
                ));
            }
        }

        for index in 0..3 {
            view.nodes.push(node(
                serial_id(index),
                Some(root_id()),
                NodeRole::Endpoint,
                format!("Serial Port {index}"),
            ));
        }

        view
    }
}
