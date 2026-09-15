mod support;

use std::collections::BTreeSet;

use se_config::definition::MachineDefinition;
use se_config::diagnostic::{DiagnosticSeverity, DiagnosticTarget};
use se_config::draft::Edit;
use se_config::id::{DeviceKindId, MachineModelId, NodeId, PropertyId};
use se_config::value::PropertyValue;
use se_config::view::{ConfigurationView, NodeRole, TopologyNode};

use support::{
    FakeMachine, bank_id, bank_size_id, cdrom_id, controller_id, cpu_id, device_id, disk_id,
    root_id, serial_id, slot_id,
};

fn find_node<'a>(view: &'a ConfigurationView, id: &NodeId) -> Option<&'a TopologyNode> {
    view.nodes.iter().find(|node| &node.id == id)
}

fn has_error(view: &ConfigurationView, code: &str, target: DiagnosticTarget) -> bool {
    view.diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == DiagnosticSeverity::Error
            && diagnostic.code == code
            && diagnostic.target == target
    })
}

#[test]
fn default_draft_resolves_the_full_fake_machine() {
    let machine = FakeMachine;
    let draft = machine.default_draft();
    let view = machine.resolve(&draft);

    assert_eq!(draft.model, machine.model_id());
    assert_eq!(view.model, machine.model_id());
    assert!(view.diagnostics.is_empty());

    for index in 0..2 {
        assert_eq!(
            find_node(&view, &cpu_id(index)).map(|node| node.role),
            Some(NodeRole::Slot)
        );
    }
    for index in 0..5 {
        assert_eq!(
            find_node(&view, &bank_id(index)).map(|node| node.role),
            Some(NodeRole::Component)
        );
    }
    for index in 0..2 {
        assert_eq!(
            find_node(&view, &controller_id(index)).map(|node| node.role),
            Some(NodeRole::Component)
        );
    }
    for index in 0..3 {
        assert_eq!(
            find_node(&view, &serial_id(index)).map(|node| node.role),
            Some(NodeRole::Endpoint)
        );
    }
    for (controller, slot_count) in [3, 2].into_iter().enumerate() {
        for slot in 0..slot_count {
            let slot_node = find_node(&view, &slot_id(controller, slot));
            assert_eq!(
                slot_node.map(|node| node.parent.clone()),
                Some(Some(controller_id(controller)))
            );
            assert!(slot_node.is_some_and(|node| node.attachment.is_some()));
        }
    }
    assert!(find_node(&view, &NodeId("gio".into())).is_none());
}

#[test]
fn resolved_nodes_use_explicit_parents_in_parent_first_order() {
    let view = FakeMachine.resolve(&FakeMachine.default_draft());
    let mut seen = BTreeSet::new();

    for node in &view.nodes {
        if let Some(parent) = &node.parent {
            assert!(seen.contains(parent));
        } else {
            assert_eq!(node.id, root_id());
        }
        assert!(seen.insert(node.id.clone()));
    }
}

#[test]
fn property_edits_update_draft_and_next_view() {
    let mut draft = FakeMachine.default_draft();
    let property = bank_size_id(2);
    draft.apply(Edit::SetProperty {
        property: property.clone(),
        value: PropertyValue::Integer(8),
    });

    assert_eq!(
        draft.properties.get(&property),
        Some(&PropertyValue::Integer(8))
    );
    let view = FakeMachine.resolve(&draft);
    let bank = find_node(&view, &bank_id(2)).expect("memory bank must be present");
    assert_eq!(
        bank.properties
            .iter()
            .find(|item| item.id == property)
            .map(|item| &item.value),
        Some(&PropertyValue::Integer(8))
    );
    assert!(view.diagnostics.is_empty());
}

#[test]
fn invalid_property_value_remains_visible_with_a_targeted_error() {
    let mut draft = FakeMachine.default_draft();
    let property = bank_size_id(0);
    draft.apply(Edit::SetProperty {
        property: property.clone(),
        value: PropertyValue::Integer(114_514),
    });

    assert_eq!(
        draft.properties.get(&property),
        Some(&PropertyValue::Integer(114_514))
    );
    let view = FakeMachine.resolve(&draft);
    let bank = find_node(&view, &bank_id(0)).expect("memory bank must remain visible");
    assert_eq!(
        bank.properties
            .iter()
            .find(|item| item.id == property)
            .map(|item| &item.value),
        Some(&PropertyValue::Integer(114_514))
    );
    assert!(has_error(
        &view,
        "fake.memory.invalid-size",
        DiagnosticTarget::Property(property)
    ));
}

#[test]
fn invalid_memory_does_not_satisfy_populated_bank_constraint() {
    let mut draft = FakeMachine.default_draft();
    let property = bank_size_id(0);
    draft.apply(Edit::SetProperty {
        property: property.clone(),
        value: PropertyValue::Integer(114_514),
    });

    let view = FakeMachine.resolve(&draft);
    assert!(has_error(
        &view,
        "fake.memory.invalid-size",
        DiagnosticTarget::Property(property)
    ));
    assert!(has_error(
        &view,
        "fake.memory.no-installed-bank",
        DiagnosticTarget::Node(root_id())
    ));
    for index in 0..5 {
        assert!(find_node(&view, &bank_id(index)).is_some());
    }
}

#[test]
fn empty_memory_reports_a_cross_property_error_without_losing_topology() {
    let mut draft = FakeMachine.default_draft();
    for index in 0..5 {
        draft.apply(Edit::SetProperty {
            property: bank_size_id(index),
            value: PropertyValue::Integer(0),
        });
    }

    let view = FakeMachine.resolve(&draft);
    assert!(has_error(
        &view,
        "fake.memory.no-installed-bank",
        DiagnosticTarget::Node(root_id())
    ));
    for index in 0..5 {
        assert!(find_node(&view, &bank_id(index)).is_some());
    }
}

#[test]
fn attachment_edits_create_replace_and_remove_device_nodes() {
    let mut draft = FakeMachine.default_draft();
    let slot = slot_id(0, 1);
    let device_node = device_id(0, 1);

    let empty = FakeMachine.resolve(&draft);
    assert!(find_node(&empty, &device_node).is_none());
    assert_eq!(
        find_node(&empty, &slot)
            .and_then(|node| node.attachment.as_ref())
            .and_then(|attachment| attachment.current.as_ref()),
        None
    );

    draft.apply(Edit::SetAttachment {
        slot: slot.clone(),
        device: Some(disk_id()),
    });
    let disk = FakeMachine.resolve(&draft);
    assert_eq!(
        find_node(&disk, &device_node).map(|node| (&node.parent, node.label.as_str())),
        Some((&Some(slot.clone()), "Disk"))
    );

    draft.apply(Edit::SetAttachment {
        slot: slot.clone(),
        device: Some(cdrom_id()),
    });
    let cdrom = FakeMachine.resolve(&draft);
    assert_eq!(draft.attachments.get(&slot), Some(&cdrom_id()));
    assert_eq!(
        find_node(&cdrom, &device_node).map(|node| node.label.as_str()),
        Some("CD-ROM")
    );

    draft.apply(Edit::SetAttachment {
        slot: slot.clone(),
        device: None,
    });
    let detached = FakeMachine.resolve(&draft);
    assert!(!draft.attachments.contains_key(&slot));
    assert!(find_node(&detached, &device_node).is_none());
}

#[test]
fn unsupported_attachment_stays_selected_and_reports_an_error() {
    let mut draft = FakeMachine.default_draft();
    let slot = slot_id(0, 0);
    draft.apply(Edit::SetAttachment {
        slot: slot.clone(),
        device: Some(cdrom_id()),
    });

    assert_eq!(draft.attachments.get(&slot), Some(&cdrom_id()));
    let view = FakeMachine.resolve(&draft);
    assert!(has_error(
        &view,
        "fake.attachment.unsupported",
        DiagnosticTarget::Node(slot.clone())
    ));
    assert_eq!(
        find_node(&view, &slot)
            .and_then(|node| node.attachment.as_ref())
            .and_then(|attachment| attachment.current.as_ref()),
        Some(&cdrom_id())
    );
    assert_eq!(
        find_node(&view, &device_id(0, 0)).map(|node| node.role),
        Some(NodeRole::Device)
    );
}

#[test]
fn unknown_device_kind_stays_visible_and_reports_an_error() {
    let mut draft = FakeMachine.default_draft();
    let slot = slot_id(1, 1);
    let unknown = DeviceKindId("unlisted-device".into());
    draft.apply(Edit::SetAttachment {
        slot: slot.clone(),
        device: Some(unknown.clone()),
    });

    let view = FakeMachine.resolve(&draft);
    assert_eq!(draft.attachments.get(&slot), Some(&unknown));
    assert!(has_error(
        &view,
        "fake.attachment.unsupported",
        DiagnosticTarget::Node(slot)
    ));
    assert!(find_node(&view, &device_id(1, 1)).is_some());
}

#[test]
fn unknown_property_produces_a_diagnostic_without_hiding_known_nodes() {
    let mut draft = FakeMachine.default_draft();
    let unknown = PropertyId("capacity:unknown".into());
    draft.apply(Edit::SetProperty {
        property: unknown.clone(),
        value: PropertyValue::Text("invalid".into()),
    });

    let view = FakeMachine.resolve(&draft);
    assert!(draft.properties.contains_key(&unknown));
    assert!(has_error(
        &view,
        "fake.property.unknown",
        DiagnosticTarget::Property(unknown)
    ));
    assert!(find_node(&view, &bank_id(0)).is_some());
}

#[test]
fn missing_required_property_uses_its_default_and_reports_an_error() {
    let mut draft = FakeMachine.default_draft();
    let property = bank_size_id(0);
    draft.properties.remove(&property);

    let view = FakeMachine.resolve(&draft);
    assert!(!draft.properties.contains_key(&property));
    assert!(has_error(
        &view,
        "fake.property.missing",
        DiagnosticTarget::Property(property.clone())
    ));
    let bank = find_node(&view, &bank_id(0)).expect("memory bank must remain visible");
    assert_eq!(
        bank.properties
            .iter()
            .find(|item| item.id == property)
            .map(|item| &item.value),
        Some(&PropertyValue::Integer(4))
    );
}

#[test]
fn mismatched_model_reports_error_without_losing_topology() {
    let mut draft = FakeMachine.default_draft();
    draft.model = MachineModelId("another-machine".into());

    let view = FakeMachine.resolve(&draft);
    assert_eq!(view.model, FakeMachine.model_id());
    assert!(has_error(
        &view,
        "fake.model.mismatch",
        DiagnosticTarget::Global
    ));
    assert!(find_node(&view, &root_id()).is_some());
    assert!(find_node(&view, &bank_id(0)).is_some());
    assert!(find_node(&view, &slot_id(1, 1)).is_some());
}

#[test]
fn unknown_slot_produces_a_diagnostic_without_hiding_known_nodes() {
    let mut draft = FakeMachine.default_draft();
    let unknown = NodeId("bay:unknown".into());
    draft.apply(Edit::SetAttachment {
        slot: unknown.clone(),
        device: Some(disk_id()),
    });

    let view = FakeMachine.resolve(&draft);
    assert!(draft.attachments.contains_key(&unknown));
    assert!(has_error(
        &view,
        "fake.attachment.unknown-slot",
        DiagnosticTarget::Node(unknown)
    ));
    assert!(find_node(&view, &slot_id(0, 0)).is_some());
}

#[test]
fn machine_draft_round_trips_through_serde() {
    let mut draft = FakeMachine.default_draft();
    draft.apply(Edit::SetProperty {
        property: bank_size_id(3),
        value: PropertyValue::Integer(8),
    });
    draft.apply(Edit::SetAttachment {
        slot: slot_id(1, 1),
        device: Some(cdrom_id()),
    });

    let encoded = serde_json::to_string(&draft).expect("draft serialization must succeed");
    let decoded = serde_json::from_str(&encoded).expect("draft deserialization must succeed");
    assert_eq!(draft, decoded);
}
