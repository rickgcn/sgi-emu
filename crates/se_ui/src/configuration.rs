//! Projection of generic machine views and edit intents across the Qt boundary.

use se_config::diagnostic::{DiagnosticSeverity, DiagnosticTarget};
use se_config::draft::Edit;
use se_config::id::{DeviceKindId, NodeId, PropertyId};
use se_config::value::PropertyValue;
use se_config::view::{ConfigurationView, NodeRole, PathKind, PropertyEditor};

use crate::bridge::ffi::{
    MachineChoiceDto, MachineConfigurationEditDto, MachineConfigurationViewDto,
    MachineDeviceChoiceDto, MachineDiagnosticDto, MachineNodeDto, MachinePropertyDto,
    MachinePropertyValueDto,
};

pub(crate) fn view_dto(view: ConfigurationView) -> MachineConfigurationViewDto {
    MachineConfigurationViewDto {
        success: true,
        error: String::new(),
        display_name: view.display_name,
        nodes: view
            .nodes
            .into_iter()
            .map(|node| {
                let (has_attachment, allow_empty, current_device, device_choices) =
                    match node.attachment {
                        Some(attachment) => (
                            true,
                            attachment.allow_empty,
                            attachment.current.map_or_else(String::new, |id| id.0),
                            attachment
                                .choices
                                .into_iter()
                                .map(|choice| MachineDeviceChoiceDto {
                                    id: choice.id.0,
                                    label: choice.label,
                                })
                                .collect(),
                        ),
                        None => (false, false, String::new(), Vec::new()),
                    };
                MachineNodeDto {
                    id: node.id.0,
                    parent_id: node.parent.map_or_else(String::new, |id| id.0),
                    role: match node.role {
                        NodeRole::Root => 0,
                        NodeRole::Component => 1,
                        NodeRole::Slot => 2,
                        NodeRole::Device => 3,
                        NodeRole::Endpoint => 4,
                    },
                    label: node.label,
                    properties: node
                        .properties
                        .into_iter()
                        .map(|property| {
                            let (editor, minimum, maximum, step, unit, path_kind, choices) =
                                match property.editor {
                                    PropertyEditor::Toggle => {
                                        (0, 0, 0, 0, String::new(), 0, Vec::new())
                                    }
                                    PropertyEditor::Integer {
                                        min,
                                        max,
                                        step,
                                        unit,
                                    } => {
                                        (1, min, max, step, unit.unwrap_or_default(), 0, Vec::new())
                                    }
                                    PropertyEditor::Text => {
                                        (2, 0, 0, 0, String::new(), 0, Vec::new())
                                    }
                                    PropertyEditor::Path { kind } => (
                                        3,
                                        0,
                                        0,
                                        0,
                                        String::new(),
                                        match kind {
                                            PathKind::File => 0,
                                            PathKind::Directory => 1,
                                        },
                                        Vec::new(),
                                    ),
                                    PropertyEditor::Choice { options } => (
                                        4,
                                        0,
                                        0,
                                        0,
                                        String::new(),
                                        0,
                                        options
                                            .into_iter()
                                            .map(|option| MachineChoiceDto {
                                                value: value_dto(option.value),
                                                label: option.label,
                                            })
                                            .collect(),
                                    ),
                                };
                            MachinePropertyDto {
                                id: property.id.0,
                                label: property.label,
                                value: value_dto(property.value),
                                editor,
                                minimum,
                                maximum,
                                step,
                                unit,
                                path_kind,
                                choices,
                            }
                        })
                        .collect(),
                    has_attachment,
                    allow_empty,
                    current_device,
                    device_choices,
                }
            })
            .collect(),
        diagnostics: view
            .diagnostics
            .into_iter()
            .map(|diagnostic| {
                let (target_kind, target_id) = match diagnostic.target {
                    DiagnosticTarget::Global => (0, String::new()),
                    DiagnosticTarget::Node(id) => (1, id.0),
                    DiagnosticTarget::Property(id) => (2, id.0),
                };
                MachineDiagnosticDto {
                    severity: match diagnostic.severity {
                        DiagnosticSeverity::Warning => 0,
                        DiagnosticSeverity::Error => 1,
                    },
                    target_kind,
                    target_id,
                    code: diagnostic.code,
                    message: diagnostic.message,
                }
            })
            .collect(),
    }
}

pub(crate) fn failed_view(error: impl Into<String>) -> MachineConfigurationViewDto {
    MachineConfigurationViewDto {
        success: false,
        error: error.into(),
        display_name: String::new(),
        nodes: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn value_dto(value: PropertyValue) -> MachinePropertyValueDto {
    match value {
        PropertyValue::Bool(bool_value) => MachinePropertyValueDto {
            kind: 0,
            bool_value,
            integer_value: 0,
            text_value: String::new(),
        },
        PropertyValue::Integer(integer_value) => MachinePropertyValueDto {
            kind: 1,
            bool_value: false,
            integer_value,
            text_value: String::new(),
        },
        PropertyValue::Text(text_value) => MachinePropertyValueDto {
            kind: 2,
            bool_value: false,
            integer_value: 0,
            text_value,
        },
    }
}

pub(crate) fn edit_from_dto(dto: &MachineConfigurationEditDto) -> Result<Edit, String> {
    if dto.target_id.is_empty() {
        return Err(String::from("machine edit target is empty"));
    }
    match dto.kind {
        0 => {
            let value = match dto.value.kind {
                0 => PropertyValue::Bool(dto.value.bool_value),
                1 => PropertyValue::Integer(dto.value.integer_value),
                2 => PropertyValue::Text(dto.value.text_value.clone()),
                _ => return Err(String::from("unknown machine property value kind")),
            };
            Ok(Edit::SetProperty {
                property: PropertyId(dto.target_id.clone()),
                value,
            })
        }
        1 => Ok(Edit::SetAttachment {
            slot: NodeId(dto.target_id.clone()),
            device: (!dto.device_id.is_empty()).then(|| DeviceKindId(dto.device_id.clone())),
        }),
        _ => Err(String::from("unknown machine edit kind")),
    }
}

#[cfg(test)]
mod tests {
    use se_config::diagnostic::{Diagnostic, DiagnosticSeverity, DiagnosticTarget};
    use se_config::draft::Edit;
    use se_config::id::{DeviceKindId, MachineModelId, NodeId, PropertyId};
    use se_config::value::PropertyValue;
    use se_config::view::{
        AttachmentView, ChoiceOption, ConfigurationView, DeviceChoice, NodeRole, PathKind,
        PropertyEditor, PropertyView, TopologyNode,
    };

    use super::{edit_from_dto, view_dto};
    use crate::bridge::ffi::{MachineConfigurationEditDto, MachinePropertyValueDto};

    #[test]
    fn bridge_preserves_all_editor_metadata_and_diagnostics() {
        let property = |id: &str, value, editor| PropertyView {
            id: PropertyId(id.into()),
            label: id.into(),
            value,
            editor,
        };
        let view = ConfigurationView {
            model: MachineModelId(String::from("example")),
            display_name: String::from("Example Machine"),
            nodes: vec![
                TopologyNode {
                    id: NodeId(String::from("root")),
                    parent: None,
                    role: NodeRole::Root,
                    label: String::from("Root"),
                    properties: Vec::new(),
                    attachment: None,
                },
                TopologyNode {
                    id: NodeId(String::from("slot")),
                    parent: Some(NodeId(String::from("root"))),
                    role: NodeRole::Slot,
                    label: String::from("Slot"),
                    properties: vec![
                        property("toggle", PropertyValue::Bool(true), PropertyEditor::Toggle),
                        property(
                            "integer",
                            PropertyValue::Integer(7),
                            PropertyEditor::Integer {
                                min: 0,
                                max: 10,
                                step: 2,
                                unit: Some(String::from("MiB")),
                            },
                        ),
                        property(
                            "text",
                            PropertyValue::Text(String::from("value")),
                            PropertyEditor::Text,
                        ),
                        property(
                            "path",
                            PropertyValue::Text(String::from("image.bin")),
                            PropertyEditor::Path {
                                kind: PathKind::File,
                            },
                        ),
                        property(
                            "choice",
                            PropertyValue::Integer(2),
                            PropertyEditor::Choice {
                                options: vec![ChoiceOption {
                                    value: PropertyValue::Integer(2),
                                    label: String::from("Two"),
                                }],
                            },
                        ),
                    ],
                    attachment: Some(AttachmentView {
                        allow_empty: true,
                        current: Some(DeviceKindId(String::from("device"))),
                        choices: vec![DeviceChoice {
                            id: DeviceKindId(String::from("device")),
                            label: String::from("Device"),
                        }],
                    }),
                },
            ],
            diagnostics: vec![Diagnostic {
                severity: DiagnosticSeverity::Error,
                code: String::from("example.error"),
                target: DiagnosticTarget::Property(PropertyId(String::from("path"))),
                message: String::from("Missing image"),
            }],
        };
        let dto = view_dto(view);
        assert!(dto.success);
        assert_eq!(dto.display_name, "Example Machine");
        let node = &dto.nodes[1];
        assert_eq!(node.parent_id, "root");
        assert_eq!(node.role, 2);
        assert!(node.has_attachment && node.allow_empty);
        assert_eq!(node.current_device, "device");
        assert_eq!(node.device_choices[0].id, "device");
        assert_eq!(
            node.properties
                .iter()
                .map(|property| property.editor)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
        assert!(node.properties[0].value.bool_value);
        assert_eq!(node.properties[1].value.integer_value, 7);
        assert_eq!(node.properties[1].maximum, 10);
        assert_eq!(node.properties[1].unit, "MiB");
        assert_eq!(node.properties[2].value.text_value, "value");
        assert_eq!(node.properties[3].value.text_value, "image.bin");
        assert_eq!(node.properties[4].choices[0].value.integer_value, 2);
        assert_eq!(dto.diagnostics[0].severity, 1);
        assert_eq!(dto.diagnostics[0].target_kind, 2);
        assert_eq!(dto.diagnostics[0].target_id, "path");
    }

    #[test]
    fn attachment_detach_and_malformed_edits_have_explicit_results() {
        let dto = MachineConfigurationEditDto {
            kind: 1,
            target_id: String::from("slot"),
            value: MachinePropertyValueDto {
                kind: 0,
                bool_value: false,
                integer_value: 0,
                text_value: String::new(),
            },
            device_id: String::new(),
        };
        assert_eq!(
            edit_from_dto(&dto),
            Ok(Edit::SetAttachment {
                slot: NodeId(String::from("slot")),
                device: None,
            })
        );
        let mut invalid = dto;
        invalid.kind = 255;
        assert!(edit_from_dto(&invalid).is_err());
    }
}
