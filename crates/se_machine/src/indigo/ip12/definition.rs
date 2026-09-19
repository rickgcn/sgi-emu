//! Editable configuration topology and typed build-plan analysis for the SGI Indigo IP12.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use se_config::definition::MachineDefinition;
use se_config::diagnostic::{Diagnostic, DiagnosticSeverity, DiagnosticTarget};
use se_config::draft::MachineDraft;
use se_config::id::{DeviceKindId, MachineModelId, NodeId, PropertyId};
use se_config::value::PropertyValue;
use se_config::view::{
    AttachmentView, ChoiceOption, ConfigurationView, DeviceChoice, NodeRole, PathKind,
    PropertyEditor, PropertyView, TopologyNode,
};
use se_core::storage::StorageAccess;
use se_device::gio::GioSlot;
use se_float::backend::Backend;

use super::Ip12MemoryConfiguration;
use super::plan::{
    GioAttachment, GioDevice, Ip12BuildPlan, Ip12CompileError, Ip12Peripheral, Ip12Port,
    Ip12PortAttachment, ScsiAttachment, ScsiDevice,
};
use crate::resource::{ResourceId, ResourceKind, ResourceRequirement, ResourceRequirements};

const MODEL: &str = "indigo-ip12";
const GRAPHICS_SLOT: &str = "gio.0.slot.graphics";
const LG1: &str = "sgi.gio.lg1";
const SCSI_DISK: &str = "scsi.disk";
const SCSI_CDROM: &str = "scsi.cdrom";
const SGI_KEYBOARD: &str = "sgi.serial.keyboard";
const SGI_MOUSE: &str = "sgi.serial.mouse";
const VT100_TERMINAL: &str = "terminal.vt100";
const FPU_BACKEND: &str = "cpu.0.fpu.0.backend";
const FIRMWARE_PATH: &str = "firmware.0.image-path";

/// Resolves Indigo IP12 drafts and compiles valid drafts into build plans.
pub struct Ip12Definition;

impl Ip12Definition {
    /// Compiles a semantically valid draft into typed IP12 construction inputs.
    ///
    /// Paths are preserved without accessing the host filesystem. Missing or
    /// invalid draft values return the same errors that resolution reports.
    ///
    /// # Errors
    ///
    /// Returns [`Ip12CompileError`] when the draft has any semantic error.
    pub fn compile(&self, draft: &MachineDraft) -> Result<Ip12BuildPlan, Ip12CompileError> {
        let analysis = self.analyze(draft);
        let Analysis {
            view,
            backend,
            memory,
            gio,
            scsi,
            port_attachments,
            resources,
        } = analysis;
        let errors: Vec<_> = view
            .diagnostics
            .into_iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .collect();
        if !errors.is_empty() {
            return Err(Ip12CompileError {
                diagnostics: errors,
            });
        }
        let floating_point_backend =
            backend.expect("error-free IP12 analysis must produce an FPU backend");
        let memory = memory.expect("error-free IP12 analysis must produce a memory configuration");
        Ok(Ip12BuildPlan::new(
            floating_point_backend,
            memory,
            ResourceId::new("firmware.0.image"),
            gio,
            scsi,
            port_attachments,
            resources,
        ))
    }

    fn analyze(&self, draft: &MachineDraft) -> Analysis {
        let mut projection = Projection::new(draft, self.model_id(), self.display_name());
        projection.cpu();
        projection.memory();
        projection.gio();
        projection.scsi();
        projection.serial();
        projection.parallel_and_ethernet();
        projection.firmware();
        projection.finish()
    }
}

impl MachineDefinition for Ip12Definition {
    fn model_id(&self) -> MachineModelId {
        MachineModelId(MODEL.into())
    }

    fn display_name(&self) -> &str {
        "IRIS Indigo (IP12)"
    }

    fn default_draft(&self) -> MachineDraft {
        MachineDraft {
            model: self.model_id(),
            properties: BTreeMap::from([
                (
                    property_id(FPU_BACKEND),
                    PropertyValue::Text("softfloat".into()),
                ),
                (memory_property('a'), PropertyValue::Integer(2)),
                (memory_property('b'), PropertyValue::Integer(0)),
                (memory_property('c'), PropertyValue::Integer(0)),
                (
                    property_id(FIRMWARE_PATH),
                    PropertyValue::Text(String::new()),
                ),
            ]),
            attachments: BTreeMap::from([
                (node_id(GRAPHICS_SLOT), device_kind(LG1)),
                (port_node(Ip12Port::Keyboard), device_kind(SGI_KEYBOARD)),
                (port_node(Ip12Port::Mouse), device_kind(SGI_MOUSE)),
                (port_node(Ip12Port::SerialA), device_kind(VT100_TERMINAL)),
                (port_node(Ip12Port::SerialB), device_kind(VT100_TERMINAL)),
            ]),
        }
    }

    fn resolve(&self, draft: &MachineDraft) -> ConfigurationView {
        self.analyze(draft).view
    }
}

struct Analysis {
    view: ConfigurationView,
    backend: Option<Backend>,
    memory: Option<Ip12MemoryConfiguration>,
    gio: Vec<GioAttachment>,
    scsi: Vec<ScsiAttachment>,
    port_attachments: Vec<Ip12PortAttachment>,
    resources: ResourceRequirements,
}

struct Projection<'a> {
    draft: &'a MachineDraft,
    view: ConfigurationView,
    known_properties: BTreeSet<PropertyId>,
    known_slots: BTreeSet<NodeId>,
    backend: Option<Backend>,
    memory: Option<Ip12MemoryConfiguration>,
    gio: Vec<GioAttachment>,
    scsi: Vec<ScsiAttachment>,
    port_attachments: Vec<Ip12PortAttachment>,
    resources: ResourceRequirements,
}

impl<'a> Projection<'a> {
    fn new(draft: &'a MachineDraft, model: MachineModelId, display_name: &str) -> Self {
        let mut view = ConfigurationView {
            model: model.clone(),
            display_name: display_name.into(),
            nodes: vec![node(node_id(MODEL), None, NodeRole::Root, display_name)],
            diagnostics: Vec::new(),
        };
        if draft.model != model {
            view.diagnostics.push(error(
                "ip12.model.mismatch",
                DiagnosticTarget::Global,
                "The draft belongs to a different machine model.",
            ));
        }
        Self {
            draft,
            view,
            known_properties: BTreeSet::new(),
            known_slots: BTreeSet::new(),
            backend: None,
            memory: None,
            gio: Vec::new(),
            scsi: Vec::new(),
            port_attachments: Vec::new(),
            resources: ResourceRequirements::default(),
        }
    }

    fn finish(mut self) -> Analysis {
        for property in self.draft.properties.keys() {
            if !self.known_properties.contains(property) {
                self.report(
                    "ip12.property.unknown",
                    DiagnosticTarget::Property(property.clone()),
                    "This property is not defined by the IP12.",
                );
            }
        }
        for slot in self.draft.attachments.keys() {
            if !self.known_slots.contains(slot) {
                self.report(
                    "ip12.attachment.unknown-slot",
                    DiagnosticTarget::Node(slot.clone()),
                    "This slot does not accept a configurable device on the IP12.",
                );
            }
        }
        Analysis {
            view: self.view,
            backend: self.backend,
            memory: self.memory,
            gio: self.gio,
            scsi: self.scsi,
            port_attachments: self.port_attachments,
            resources: self.resources,
        }
    }

    fn report(&mut self, code: &str, target: DiagnosticTarget, message: &str) {
        self.view.diagnostics.push(error(code, target, message));
    }

    fn required_value(&mut self, property: &PropertyId, default: PropertyValue) -> PropertyValue {
        self.known_properties.insert(property.clone());
        match self.draft.properties.get(property) {
            Some(value) => value.clone(),
            None => {
                self.report(
                    "ip12.property.missing",
                    DiagnosticTarget::Property(property.clone()),
                    "This required property is missing from the draft.",
                );
                default
            }
        }
    }

    fn cpu(&mut self) {
        self.view.nodes.push(node(
            node_id("cpu.0"),
            Some(node_id(MODEL)),
            NodeRole::Component,
            "CPU 0 — MIPS R3000A @ 33 MHz",
        ));
        let property = property_id(FPU_BACKEND);
        let value = self.required_value(&property, PropertyValue::Text("softfloat".into()));
        match &value {
            PropertyValue::Text(backend) if backend == "softfloat" => {
                self.backend = Some(Backend::SoftFloat);
            }
            PropertyValue::Text(backend) if backend == "native" => {
                self.backend = Some(Backend::Native);
            }
            PropertyValue::Text(_) => self.report(
                "ip12.property.unsupported-value",
                DiagnosticTarget::Property(property.clone()),
                "The FPU backend must be softfloat or native.",
            ),
            _ => self.report(
                "ip12.property.invalid-type",
                DiagnosticTarget::Property(property.clone()),
                "The FPU backend must be text.",
            ),
        }
        let mut fpu = node(
            node_id("cpu.0.fpu.0"),
            Some(node_id("cpu.0")),
            NodeRole::Component,
            "R3010 FPU",
        );
        fpu.properties.push(PropertyView {
            id: property,
            label: "Emulation backend".into(),
            value,
            editor: PropertyEditor::Choice {
                options: vec![
                    choice(PropertyValue::Text("softfloat".into()), "SoftFloat"),
                    choice(PropertyValue::Text("native".into()), "Native"),
                ],
            },
        });
        self.view.nodes.push(fpu);
    }

    fn memory(&mut self) {
        self.view.nodes.push(node(
            node_id("memory"),
            Some(node_id(MODEL)),
            NodeRole::Component,
            "Memory",
        ));
        let mut populated = false;
        let mut simm_mib = [0; 3];
        let mut all_sizes_valid = true;
        for (index, bank) in ['a', 'b', 'c'].into_iter().enumerate() {
            let bank_id = memory_bank(bank);
            let property = memory_property(bank);
            let default = if bank == 'a' { 2 } else { 0 };
            let value = self.required_value(&property, PropertyValue::Integer(default));
            let valid_size = match &value {
                PropertyValue::Integer(size @ (0 | 2 | 4 | 8)) => Some(*size),
                PropertyValue::Integer(_) => {
                    self.report(
                        "ip12.property.unsupported-value",
                        DiagnosticTarget::Property(property.clone()),
                        "A SIMM must be empty or have 2, 4, or 8 MiB.",
                    );
                    None
                }
                _ => {
                    self.report(
                        "ip12.property.invalid-type",
                        DiagnosticTarget::Property(property.clone()),
                        "A SIMM capacity must be an integer.",
                    );
                    None
                }
            };
            if valid_size.is_some_and(|size| size > 0) {
                populated = true;
            }
            if let Some(size) = valid_size {
                simm_mib[index] = size as u8;
            } else {
                all_sizes_valid = false;
            }
            let mut bank_node = node(
                bank_id.clone(),
                Some(node_id("memory")),
                NodeRole::Component,
                format!("Bank {}", bank.to_ascii_uppercase()),
            );
            bank_node.properties.push(PropertyView {
                id: property,
                label: "SIMM capacity".into(),
                value,
                editor: PropertyEditor::Choice {
                    options: [0, 2, 4, 8]
                        .into_iter()
                        .map(|size| {
                            choice(
                                PropertyValue::Integer(size),
                                if size == 0 {
                                    "Empty".into()
                                } else {
                                    format!("{size} MiB")
                                },
                            )
                        })
                        .collect(),
                },
            });
            self.view.nodes.push(bank_node);
            for socket in 0..4 {
                let socket_id = memory_socket(bank, socket);
                self.view.nodes.push(node(
                    socket_id.clone(),
                    Some(bank_id.clone()),
                    NodeRole::Slot,
                    format!("SIMM Socket {socket}"),
                ));
                if let Some(size) = valid_size.filter(|size| *size > 0) {
                    self.view.nodes.push(node(
                        memory_module(bank, socket),
                        Some(socket_id),
                        NodeRole::Device,
                        format!("{size} MiB SIMM"),
                    ));
                }
            }
        }
        if !populated {
            self.report(
                "ip12.memory.no-installed-bank",
                DiagnosticTarget::Node(node_id("memory")),
                "At least one memory bank must contain a supported SIMM capacity.",
            );
        }
        if populated && all_sizes_valid {
            self.memory = Ip12MemoryConfiguration::try_from_simm_mib(simm_mib).ok();
        }
    }

    fn gio(&mut self) {
        self.view.nodes.push(node(
            node_id("gio.0"),
            Some(node_id(MODEL)),
            NodeRole::Component,
            "GIO",
        ));
        for (slot, label) in [("gio.0.slot.0", "Slot 0"), ("gio.0.slot.1", "Slot 1")] {
            self.view.nodes.push(node(
                node_id(slot),
                Some(node_id("gio.0")),
                NodeRole::Slot,
                label,
            ));
        }
        let slot = node_id(GRAPHICS_SLOT);
        self.known_slots.insert(slot.clone());
        let current = self.draft.attachments.get(&slot).cloned();
        let mut slot_node = node(
            slot.clone(),
            Some(node_id("gio.0")),
            NodeRole::Slot,
            "Graphics Slot",
        );
        slot_node.attachment = Some(AttachmentView {
            allow_empty: true,
            current: current.clone(),
            choices: vec![device_choice(LG1, "LG1 Entry Graphics")],
        });
        self.view.nodes.push(slot_node);
        if let Some(device) = current {
            let supported = device == device_kind(LG1);
            if !supported {
                self.unsupported_attachment(&slot);
            }
            let device_id = node_id("gio.0.slot.graphics.device");
            self.view.nodes.push(node(
                device_id.clone(),
                Some(slot),
                NodeRole::Device,
                if supported {
                    "LG1 Entry Graphics".into()
                } else {
                    unsupported_label(&device)
                },
            ));
            if supported {
                self.gio.push(GioAttachment {
                    slot: GioSlot::Graphics,
                    device: GioDevice::Lg1,
                });
                self.view.nodes.push(node(
                    node_id("gio.0.slot.graphics.device.video.0"),
                    Some(device_id),
                    NodeRole::Endpoint,
                    "Video Output 0",
                ));
            }
        }
    }

    fn scsi(&mut self) {
        self.view.nodes.push(node(
            node_id("scsi.0"),
            Some(node_id(MODEL)),
            NodeRole::Component,
            "SCSI Controller 0",
        ));
        let host_target = scsi_target(0);
        self.view.nodes.push(node(
            host_target.clone(),
            Some(node_id("scsi.0")),
            NodeRole::Component,
            "Target 0",
        ));
        self.view.nodes.push(node(
            scsi_host_adapter(),
            Some(host_target),
            NodeRole::Device,
            "Host Adapter",
        ));
        for target in 1..8 {
            let target_id = scsi_target(target);
            self.view.nodes.push(node(
                target_id.clone(),
                Some(node_id("scsi.0")),
                NodeRole::Component,
                format!("Target {target}"),
            ));
            for lun in 0..8 {
                let slot = scsi_lun(target, lun);
                let medium = scsi_medium(target, lun);
                self.known_properties.insert(medium.clone());
                self.known_slots.insert(slot.clone());
                let current = self.draft.attachments.get(&slot).cloned();
                let mut lun_node = node(
                    slot.clone(),
                    Some(target_id.clone()),
                    NodeRole::Slot,
                    format!("LUN {lun}"),
                );
                lun_node.attachment = Some(AttachmentView {
                    allow_empty: true,
                    current: current.clone(),
                    choices: vec![
                        device_choice(SCSI_DISK, "SCSI Disk"),
                        device_choice(SCSI_CDROM, "SCSI CD-ROM"),
                    ],
                });
                self.view.nodes.push(lun_node);
                if let Some(device) = current {
                    let supported =
                        device == device_kind(SCSI_DISK) || device == device_kind(SCSI_CDROM);
                    if !supported {
                        self.unsupported_attachment(&slot);
                    }
                    let label = if device == device_kind(SCSI_DISK) {
                        "SCSI Disk".into()
                    } else if device == device_kind(SCSI_CDROM) {
                        "SCSI CD-ROM".into()
                    } else {
                        unsupported_label(&device)
                    };
                    let mut device_node = node(
                        scsi_device(target, lun),
                        Some(slot),
                        NodeRole::Device,
                        label,
                    );
                    if supported {
                        let scsi_device = if device == device_kind(SCSI_DISK) {
                            ScsiDevice::Disk
                        } else {
                            ScsiDevice::Cdrom
                        };
                        let value = self
                            .draft
                            .properties
                            .get(&medium)
                            .cloned()
                            .unwrap_or_else(|| PropertyValue::Text(String::new()));
                        match &value {
                            PropertyValue::Text(path) if !path.trim().is_empty() => {
                                let medium_id = ResourceId::new(format!(
                                    "scsi.0.target.{target}.lun.{lun}.medium"
                                ));
                                self.resources.insert(
                                    medium_id.clone(),
                                    ResourceRequirement {
                                        path: PathBuf::from(path),
                                        kind: ResourceKind::Storage {
                                            access: scsi_storage_access(scsi_device),
                                        },
                                        origin: DiagnosticTarget::Property(medium.clone()),
                                    },
                                );
                                self.scsi.push(match scsi_device {
                                    ScsiDevice::Disk => {
                                        ScsiAttachment::disk(target as u8, lun as u8, medium_id)
                                    }
                                    ScsiDevice::Cdrom => ScsiAttachment::cdrom(
                                        target as u8,
                                        lun as u8,
                                        Some(medium_id),
                                    ),
                                });
                            }
                            // Only a removable drive may start without a medium.
                            PropertyValue::Text(_) => match scsi_device {
                                ScsiDevice::Disk => self.report(
                                    "ip12.scsi.medium-required",
                                    DiagnosticTarget::Property(medium.clone()),
                                    "Select a disk image.",
                                ),
                                ScsiDevice::Cdrom => self.scsi.push(ScsiAttachment::cdrom(
                                    target as u8,
                                    lun as u8,
                                    None,
                                )),
                            },
                            _ => self.report(
                                "ip12.property.invalid-type",
                                DiagnosticTarget::Property(medium.clone()),
                                "The medium path must be text.",
                            ),
                        }
                        device_node.properties.push(path_property(
                            medium,
                            scsi_medium_label(scsi_device),
                            value,
                        ));
                    }
                    self.view.nodes.push(device_node);
                }
            }
        }
    }

    fn serial(&mut self) {
        for controller in 0..2 {
            let controller_id = node_id(&format!("serial.{controller}"));
            self.view.nodes.push(node(
                controller_id.clone(),
                Some(node_id(MODEL)),
                NodeRole::Component,
                format!("Serial Controller {controller}"),
            ));
            let ports = if controller == 0 {
                [
                    (
                        "a",
                        Ip12Port::Keyboard,
                        "Keyboard Port",
                        SGI_KEYBOARD,
                        "SGI Keyboard",
                    ),
                    ("b", Ip12Port::Mouse, "Mouse Port", SGI_MOUSE, "SGI Mouse"),
                ]
            } else {
                [
                    (
                        "a",
                        Ip12Port::SerialA,
                        "Serial Port A",
                        VT100_TERMINAL,
                        "VT100 Terminal",
                    ),
                    (
                        "b",
                        Ip12Port::SerialB,
                        "Serial Port B",
                        VT100_TERMINAL,
                        "VT100 Terminal",
                    ),
                ]
            };
            for (channel, port, port_label, device_kind_id, device_label) in ports {
                let channel_id = node_id(&format!("serial.{controller}.channel.{channel}"));
                self.view.nodes.push(node(
                    channel_id.clone(),
                    Some(controller_id.clone()),
                    NodeRole::Component,
                    format!("Channel {}", channel.to_ascii_uppercase()),
                ));
                let port_id = port_node(port);
                self.known_slots.insert(port_id.clone());
                let current = self.draft.attachments.get(&port_id).cloned();
                let mut port_view = node(
                    port_id.clone(),
                    Some(channel_id),
                    NodeRole::Endpoint,
                    port_label,
                );
                port_view.attachment = Some(AttachmentView {
                    allow_empty: true,
                    current: current.clone(),
                    choices: vec![device_choice(device_kind_id, device_label)],
                });
                self.view.nodes.push(port_view);
                if let Some(device) = current {
                    let supported = device == device_kind(device_kind_id);
                    if !supported {
                        self.unsupported_attachment(&port_id);
                    }
                    self.view.nodes.push(node(
                        port_device_node(port),
                        Some(port_id.clone()),
                        NodeRole::Device,
                        if supported {
                            device_label.into()
                        } else {
                            unsupported_label(&device)
                        },
                    ));
                    if supported {
                        let peripheral = match port {
                            Ip12Port::Keyboard => Ip12Peripheral::SgiKeyboard,
                            Ip12Port::Mouse => Ip12Peripheral::SgiMouse,
                            Ip12Port::SerialA | Ip12Port::SerialB => Ip12Peripheral::Vt100Terminal,
                        };
                        self.port_attachments
                            .push(Ip12PortAttachment::new(port, peripheral));
                    }
                }
            }
        }
    }

    fn parallel_and_ethernet(&mut self) {
        self.view.nodes.push(node(
            node_id("parallel.0"),
            Some(node_id(MODEL)),
            NodeRole::Endpoint,
            "Parallel Port",
        ));
        self.view.nodes.push(node(
            node_id("ethernet.0"),
            Some(node_id(MODEL)),
            NodeRole::Component,
            "Ethernet Controller",
        ));
        self.view.nodes.push(node(
            node_id("ethernet.0.port.0"),
            Some(node_id("ethernet.0")),
            NodeRole::Endpoint,
            "Ethernet Port 0",
        ));
    }

    fn firmware(&mut self) {
        let property = property_id(FIRMWARE_PATH);
        self.known_properties.insert(property.clone());
        let value = self
            .draft
            .properties
            .get(&property)
            .cloned()
            .unwrap_or_else(|| PropertyValue::Text(String::new()));
        match &value {
            PropertyValue::Text(path) if !path.trim().is_empty() => {
                self.resources.insert(
                    ResourceId::new("firmware.0.image"),
                    ResourceRequirement {
                        path: PathBuf::from(path),
                        kind: ResourceKind::Bytes,
                        origin: DiagnosticTarget::Property(property.clone()),
                    },
                );
            }
            PropertyValue::Text(_) => self.report(
                "ip12.firmware.image-required",
                DiagnosticTarget::Property(property.clone()),
                "Select a PROM image.",
            ),
            _ => self.report(
                "ip12.property.invalid-type",
                DiagnosticTarget::Property(property.clone()),
                "The PROM image path must be text.",
            ),
        }
        let mut firmware = node(
            node_id("firmware.0"),
            Some(node_id(MODEL)),
            NodeRole::Component,
            "Firmware",
        );
        firmware
            .properties
            .push(path_property(property, "PROM image", value));
        self.view.nodes.push(firmware);
    }

    fn unsupported_attachment(&mut self, slot: &NodeId) {
        self.report(
            "ip12.attachment.unsupported-device",
            DiagnosticTarget::Node(slot.clone()),
            "This device kind is unsupported in this slot.",
        );
    }
}

fn node_id(value: &str) -> NodeId {
    NodeId(value.into())
}
fn property_id(value: &str) -> PropertyId {
    PropertyId(value.into())
}
fn device_kind(value: &str) -> DeviceKindId {
    DeviceKindId(value.into())
}
fn port_node(port: Ip12Port) -> NodeId {
    node_id(match port {
        Ip12Port::Keyboard => "serial.0.channel.a.port",
        Ip12Port::Mouse => "serial.0.channel.b.port",
        Ip12Port::SerialA => "serial.1.channel.a.port",
        Ip12Port::SerialB => "serial.1.channel.b.port",
    })
}
fn port_device_node(port: Ip12Port) -> NodeId {
    node_id(match port {
        Ip12Port::Keyboard => "serial.0.channel.a.device",
        Ip12Port::Mouse => "serial.0.channel.b.device",
        Ip12Port::SerialA => "serial.1.channel.a.device",
        Ip12Port::SerialB => "serial.1.channel.b.device",
    })
}
fn memory_bank(bank: char) -> NodeId {
    node_id(&format!("memory.bank.{bank}"))
}
fn memory_property(bank: char) -> PropertyId {
    property_id(&format!("memory.bank.{bank}.simm-mib"))
}
fn memory_socket(bank: char, socket: usize) -> NodeId {
    node_id(&format!("memory.bank.{bank}.socket.{socket}"))
}
fn memory_module(bank: char, socket: usize) -> NodeId {
    node_id(&format!("memory.bank.{bank}.socket.{socket}.module"))
}
fn scsi_target(target: usize) -> NodeId {
    node_id(&format!("scsi.0.target.{target}"))
}
fn scsi_host_adapter() -> NodeId {
    node_id("scsi.0.target.0.host-adapter")
}
fn scsi_lun(target: usize, lun: usize) -> NodeId {
    node_id(&format!("scsi.0.target.{target}.lun.{lun}"))
}
fn scsi_device(target: usize, lun: usize) -> NodeId {
    node_id(&format!("scsi.0.target.{target}.lun.{lun}.device"))
}
fn scsi_medium(target: usize, lun: usize) -> PropertyId {
    property_id(&format!("scsi.0.target.{target}.lun.{lun}.medium-path"))
}

/// Returns the host access this device requires from its medium.
const fn scsi_storage_access(device: ScsiDevice) -> StorageAccess {
    match device {
        ScsiDevice::Disk => StorageAccess::ReadWrite,
        ScsiDevice::Cdrom => StorageAccess::ReadOnly,
    }
}

/// Returns the settings label of this device's cold-start medium.
const fn scsi_medium_label(device: ScsiDevice) -> &'static str {
    match device {
        ScsiDevice::Disk => "Disk image",
        ScsiDevice::Cdrom => "Initial medium",
    }
}

fn node(
    id: NodeId,
    parent: Option<NodeId>,
    role: NodeRole,
    label: impl Into<String>,
) -> TopologyNode {
    TopologyNode {
        id,
        parent,
        role,
        label: label.into(),
        properties: Vec::new(),
        attachment: None,
    }
}

fn choice(value: PropertyValue, label: impl Into<String>) -> ChoiceOption {
    ChoiceOption {
        value,
        label: label.into(),
    }
}

fn device_choice(id: &str, label: &str) -> DeviceChoice {
    DeviceChoice {
        id: device_kind(id),
        label: label.into(),
    }
}

fn path_property(id: PropertyId, label: &str, value: PropertyValue) -> PropertyView {
    PropertyView {
        id,
        label: label.into(),
        value,
        editor: PropertyEditor::Path {
            kind: PathKind::File,
        },
    }
}

fn unsupported_label(device: &DeviceKindId) -> String {
    format!("Unsupported device ({})", device.0)
}

fn error(code: &str, target: DiagnosticTarget, message: &str) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Error,
        code: code.into(),
        target,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use se_config::definition::MachineDefinition;
    use se_config::diagnostic::{DiagnosticSeverity, DiagnosticTarget};
    use se_config::draft::{Edit, MachineDraft};
    use se_config::id::{DeviceKindId, MachineModelId, NodeId, PropertyId};
    use se_config::value::PropertyValue;
    use se_config::view::{ConfigurationView, NodeRole, PathKind, PropertyEditor, TopologyNode};
    use se_core::storage::StorageAccess;
    use se_device::gio::GioSlot;
    use se_float::backend::Backend;

    use crate::indigo::ip12::plan::{GioDevice, Ip12Peripheral, Ip12Port, ScsiDevice};
    use crate::resource::{ResourceId, ResourceKind};

    use super::{
        FIRMWARE_PATH, FPU_BACKEND, GRAPHICS_SLOT, Ip12Definition, LG1, MODEL, SCSI_CDROM,
        SCSI_DISK, SGI_KEYBOARD, SGI_MOUSE, VT100_TERMINAL, device_kind, memory_bank,
        memory_module, memory_property, memory_socket, node_id, port_device_node, port_node,
        property_id, scsi_device, scsi_host_adapter, scsi_lun, scsi_medium, scsi_target,
    };

    fn node<'a>(view: &'a ConfigurationView, id: &NodeId) -> &'a TopologyNode {
        view.nodes
            .iter()
            .find(|node| &node.id == id)
            .expect("node must exist")
    }

    fn contains(view: &ConfigurationView, id: &NodeId) -> bool {
        view.nodes.iter().any(|node| &node.id == id)
    }

    fn value<'a>(view: &'a ConfigurationView, property: &PropertyId) -> &'a PropertyValue {
        view.nodes
            .iter()
            .flat_map(|node| &node.properties)
            .find(|item| &item.id == property)
            .map(|item| &item.value)
            .expect("property must be visible")
    }

    fn error(view: &ConfigurationView, code: &str, target: DiagnosticTarget) -> bool {
        view.diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == DiagnosticSeverity::Error
                && diagnostic.code == code
                && diagnostic.target == target
        })
    }

    fn targeted_codes<'a>(view: &'a ConfigurationView, target: &DiagnosticTarget) -> Vec<&'a str> {
        view.diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.severity == DiagnosticSeverity::Error && &diagnostic.target == target
            })
            .map(|diagnostic| diagnostic.code.as_str())
            .collect()
    }

    fn set(draft: &mut MachineDraft, property: PropertyId, value: PropertyValue) {
        draft.apply(Edit::SetProperty { property, value });
    }

    fn attach(draft: &mut MachineDraft, slot: NodeId, device: Option<&str>) {
        draft.apply(Edit::SetAttachment {
            slot,
            device: device.map(device_kind),
        });
    }

    fn valid_draft() -> MachineDraft {
        let mut draft = Ip12Definition.default_draft();
        set(
            &mut draft,
            property_id(FIRMWARE_PATH),
            PropertyValue::Text("/definitely/not/a/real/prom.bin".into()),
        );
        draft
    }

    fn assert_compile_matches_resolution(draft: &MachineDraft) {
        let view = Ip12Definition.resolve(draft);
        let compile_error = Ip12Definition
            .compile(draft)
            .expect_err("an invalid draft must not compile");
        let view_errors: Vec<_> = view
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .cloned()
            .collect();
        assert!(!view_errors.is_empty());
        assert_eq!(compile_error.diagnostics(), view_errors);
        assert!(contains(&view, &node_id(MODEL)));
    }

    #[test]
    fn default_compile_requires_firmware_and_nonexistent_path_is_preserved() {
        let draft = Ip12Definition.default_draft();
        assert_compile_matches_resolution(&draft);
        assert!(error(
            &Ip12Definition.resolve(&draft),
            "ip12.firmware.image-required",
            DiagnosticTarget::Property(property_id(FIRMWARE_PATH))
        ));

        let draft = valid_draft();
        let plan = Ip12Definition
            .compile(&draft)
            .expect("compilation must not inspect the firmware path");
        assert_eq!(plan.floating_point_backend(), Backend::SoftFloat);
        assert_eq!(plan.memory().simm_mib(), [2, 0, 0]);
        assert_eq!(plan.firmware().as_str(), "firmware.0.image");
        assert_eq!(plan.resources().len(), 1);
        let firmware = plan
            .resources()
            .get(plan.firmware())
            .expect("the firmware resource must exist");
        assert_eq!(
            (&firmware.path, firmware.kind),
            (
                &PathBuf::from("/definitely/not/a/real/prom.bin"),
                ResourceKind::Bytes
            )
        );
        assert_eq!(
            firmware.origin,
            DiagnosticTarget::Property(property_id(FIRMWARE_PATH))
        );
        assert_eq!(plan.gio().len(), 1);
        assert_eq!(plan.gio()[0].slot, GioSlot::Graphics);
        assert_eq!(plan.gio()[0].device, GioDevice::Lg1);
        assert!(plan.scsi().is_empty());
        assert_eq!(
            plan.port_attachments()
                .iter()
                .map(|attachment| (attachment.port(), attachment.peripheral()))
                .collect::<Vec<_>>(),
            [
                (Ip12Port::Keyboard, Ip12Peripheral::SgiKeyboard),
                (Ip12Port::Mouse, Ip12Peripheral::SgiMouse),
                (Ip12Port::SerialA, Ip12Peripheral::Vt100Terminal),
                (Ip12Port::SerialB, Ip12Peripheral::Vt100Terminal),
            ]
        );
    }

    #[test]
    fn fpu_and_memory_compile_to_existing_hardware_types() {
        let mut draft = valid_draft();
        set(
            &mut draft,
            property_id(FPU_BACKEND),
            PropertyValue::Text("native".into()),
        );
        set(&mut draft, memory_property('c'), PropertyValue::Integer(8));
        let plan = Ip12Definition.compile(&draft).expect("valid draft");
        assert_eq!(plan.floating_point_backend(), Backend::Native);
        assert_eq!(plan.memory().simm_mib(), [2, 0, 8]);

        for invalid in [
            PropertyValue::Text("whatever".into()),
            PropertyValue::Bool(true),
        ] {
            set(&mut draft, property_id(FPU_BACKEND), invalid);
            assert_compile_matches_resolution(&draft);
        }
        set(
            &mut draft,
            property_id(FPU_BACKEND),
            PropertyValue::Text("softfloat".into()),
        );
        set(
            &mut draft,
            memory_property('a'),
            PropertyValue::Integer(114_514),
        );
        assert_compile_matches_resolution(&draft);
        set(&mut draft, memory_property('a'), PropertyValue::Integer(0));
        set(&mut draft, memory_property('c'), PropertyValue::Integer(0));
        assert!(error(
            &Ip12Definition.resolve(&draft),
            "ip12.memory.no-installed-bank",
            DiagnosticTarget::Node(node_id("memory"))
        ));
        assert_compile_matches_resolution(&draft);
    }

    #[test]
    fn missing_memory_fallback_stays_in_the_view() {
        let mut draft = valid_draft();
        let property = memory_property('a');
        draft.properties.remove(&property);
        let view = Ip12Definition.resolve(&draft);
        assert_eq!(value(&view, &property), &PropertyValue::Integer(2));
        assert!(error(
            &view,
            "ip12.property.missing",
            DiagnosticTarget::Property(property.clone())
        ));
        assert!(!draft.properties.contains_key(&property));
        assert_compile_matches_resolution(&draft);
    }

    #[test]
    fn detached_lg1_compiles_to_an_empty_gio_list() {
        let mut draft = valid_draft();
        attach(&mut draft, node_id(GRAPHICS_SLOT), None);
        let plan = Ip12Definition
            .compile(&draft)
            .expect("empty graphics slot is valid");
        assert!(plan.gio().is_empty());
    }

    #[test]
    fn multiple_scsi_devices_compile_with_independent_ordered_media() {
        let mut draft = valid_draft();
        let devices = [
            (1, 0, SCSI_DISK, "/this/does/not/exist.img"),
            (2, 0, SCSI_DISK, "two.img"),
            (3, 5, SCSI_DISK, "three.img"),
            (4, 0, SCSI_CDROM, "install.iso"),
        ];
        for (target, lun, kind, path) in devices.into_iter().rev() {
            attach(&mut draft, scsi_lun(target, lun), Some(kind));
            set(
                &mut draft,
                scsi_medium(target, lun),
                PropertyValue::Text(path.into()),
            );
        }
        let plan = Ip12Definition
            .compile(&draft)
            .expect("paths are not opened");
        assert_eq!(plan.scsi().len(), 4);
        assert_eq!(plan.resources().len(), 5);
        for (attachment, (target, lun, kind, path)) in plan.scsi().iter().zip(devices) {
            assert_eq!(
                (attachment.target, attachment.lun),
                (target as u8, lun as u8)
            );
            assert_eq!(
                attachment.device,
                if kind == SCSI_DISK {
                    ScsiDevice::Disk
                } else {
                    ScsiDevice::Cdrom
                }
            );
            assert_eq!(
                attachment.medium().unwrap().as_str(),
                format!("scsi.0.target.{target}.lun.{lun}.medium")
            );
            let resource = plan
                .resources()
                .get(attachment.medium().unwrap())
                .expect("each attachment needs a distinct medium role");
            assert_eq!(resource.path, PathBuf::from(path));
            assert_eq!(
                resource.kind,
                ResourceKind::Storage {
                    access: if kind == SCSI_DISK {
                        StorageAccess::ReadWrite
                    } else {
                        StorageAccess::ReadOnly
                    }
                }
            );
            assert_eq!(
                resource.origin,
                DiagnosticTarget::Property(scsi_medium(target, lun))
            );
        }
        let roles: Vec<_> = plan.resources().iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(roles[0], "firmware.0.image");
        assert_eq!(roles[1], "scsi.0.target.1.lun.0.medium");
        assert_eq!(
            plan,
            Ip12Definition.compile(&draft).expect("deterministic plan")
        );
    }

    #[test]
    fn detached_stale_medium_is_ignored_until_reattached() {
        let mut draft = valid_draft();
        let slot = scsi_lun(2, 3);
        let medium = scsi_medium(2, 3);
        attach(&mut draft, slot.clone(), Some(SCSI_DISK));
        set(
            &mut draft,
            medium.clone(),
            PropertyValue::Text("stale.img".into()),
        );
        let medium_id = ResourceId::new("scsi.0.target.2.lun.3.medium");
        assert!(Ip12Definition.compile(&draft).is_ok());
        attach(&mut draft, slot.clone(), None);
        let detached = Ip12Definition
            .compile(&draft)
            .expect("stale path is inactive");
        assert!(draft.properties.contains_key(&medium));
        assert!(detached.scsi().is_empty());
        assert!(detached.resources().get(&medium_id).is_none());
        set(&mut draft, medium.clone(), PropertyValue::Integer(99));
        assert!(Ip12Definition.compile(&draft).is_ok());
        set(&mut draft, medium, PropertyValue::Text("stale.img".into()));
        attach(&mut draft, slot, Some(SCSI_DISK));
        let reattached = Ip12Definition
            .compile(&draft)
            .expect("stale path is reused");
        assert_eq!(reattached.scsi().len(), 1);
        assert_eq!(reattached.scsi()[0].medium(), Some(&medium_id));
        assert_eq!(
            reattached
                .resources()
                .get(&medium_id)
                .map(|item| &item.path),
            Some(&PathBuf::from("stale.img"))
        );
    }

    #[test]
    fn attached_scsi_medium_must_be_text() {
        for kind in [SCSI_DISK, SCSI_CDROM] {
            let mut draft = valid_draft();
            attach(&mut draft, scsi_lun(1, 0), Some(kind));
            set(&mut draft, scsi_medium(1, 0), PropertyValue::Integer(3));
            assert_compile_matches_resolution(&draft);
        }
    }

    #[test]
    fn duplicate_host_paths_have_distinct_resource_roles() {
        let mut draft = valid_draft();
        for target in [1, 2] {
            attach(&mut draft, scsi_lun(target, 0), Some(SCSI_DISK));
            set(
                &mut draft,
                scsi_medium(target, 0),
                PropertyValue::Text("same.img".into()),
            );
        }
        let plan = Ip12Definition
            .compile(&draft)
            .expect("path collisions are checked later");
        assert_ne!(plan.scsi()[0].medium(), plan.scsi()[1].medium());
        for attachment in plan.scsi() {
            assert_eq!(
                plan.resources()
                    .get(attachment.medium().unwrap())
                    .map(|item| &item.path),
                Some(&PathBuf::from("same.img"))
            );
        }
    }

    #[test]
    fn invalid_draft_reasons_match_resolved_diagnostics() {
        let mut model = valid_draft();
        model.model = MachineModelId("indy".into());
        assert_compile_matches_resolution(&model);

        let mut unknown_property = valid_draft();
        set(
            &mut unknown_property,
            property_id("unknown.property"),
            PropertyValue::Bool(true),
        );
        assert_compile_matches_resolution(&unknown_property);

        let mut unknown_slot = valid_draft();
        attach(&mut unknown_slot, node_id("gio.0.slot.99"), Some(LG1));
        assert_compile_matches_resolution(&unknown_slot);

        let mut unsupported_device = valid_draft();
        attach(
            &mut unsupported_device,
            scsi_lun(1, 0),
            Some("unsupported.device"),
        );
        assert_compile_matches_resolution(&unsupported_device);

        let mut wrong_type = valid_draft();
        set(
            &mut wrong_type,
            memory_property('a'),
            PropertyValue::Text("2".into()),
        );
        assert_compile_matches_resolution(&wrong_type);

        let mut unsupported_value = valid_draft();
        set(
            &mut unsupported_value,
            memory_property('a'),
            PropertyValue::Integer(114_514),
        );
        assert_compile_matches_resolution(&unsupported_value);
    }

    #[test]
    fn default_draft_projects_the_ip12_topology() {
        let definition = Ip12Definition;
        let draft = definition.default_draft();
        let view = definition.resolve(&draft);
        assert_eq!(draft.model, MachineModelId(MODEL.into()));
        assert_eq!(view.model, draft.model);
        assert_eq!(draft.attachments.len(), 5);
        assert_eq!(
            draft.attachments.get(&node_id(GRAPHICS_SLOT)),
            Some(&device_kind(LG1))
        );
        assert!(view.nodes.iter().all(|node| node.id != node_id("pic1")));

        assert_eq!(node(&view, &node_id(MODEL)).role, NodeRole::Root);
        assert_eq!(
            node(&view, &node_id("cpu.0")).label,
            "CPU 0 — MIPS R3000A @ 33 MHz"
        );
        assert_eq!(
            node(&view, &node_id("cpu.0.fpu.0")).role,
            NodeRole::Component
        );
        assert_eq!(
            value(&view, &property_id(FPU_BACKEND)),
            &PropertyValue::Text("softfloat".into())
        );

        for bank in ['a', 'b', 'c'] {
            assert_eq!(node(&view, &memory_bank(bank)).role, NodeRole::Component);
            let expected = if bank == 'a' { 2 } else { 0 };
            assert_eq!(
                value(&view, &memory_property(bank)),
                &PropertyValue::Integer(expected)
            );
            for socket in 0..4 {
                let socket_node = node(&view, &memory_socket(bank, socket));
                assert_eq!(socket_node.role, NodeRole::Slot);
                assert!(socket_node.attachment.is_none());
                assert_eq!(contains(&view, &memory_module(bank, socket)), bank == 'a');
            }
        }
        let bank_editor = &node(&view, &memory_bank('a')).properties[0].editor;
        assert!(
            matches!(bank_editor, PropertyEditor::Choice { options } if options.iter().map(|option| &option.value).collect::<Vec<_>>() == vec![&PropertyValue::Integer(0), &PropertyValue::Integer(2), &PropertyValue::Integer(4), &PropertyValue::Integer(8)])
        );

        for slot in ["gio.0.slot.0", "gio.0.slot.1", GRAPHICS_SLOT] {
            assert_eq!(node(&view, &node_id(slot)).role, NodeRole::Slot);
        }
        let graphics = node(&view, &node_id(GRAPHICS_SLOT))
            .attachment
            .as_ref()
            .expect("graphics slot must be editable");
        assert_eq!(graphics.current, Some(device_kind(LG1)));
        assert_eq!(graphics.choices.len(), 1);
        assert_eq!(
            node(&view, &node_id("gio.0.slot.graphics.device")).role,
            NodeRole::Device
        );
        assert_eq!(
            node(&view, &node_id("gio.0.slot.graphics.device.video.0")).role,
            NodeRole::Endpoint
        );

        assert_eq!(node(&view, &node_id("scsi.0")).role, NodeRole::Component);
        let host_target = node(&view, &scsi_target(0));
        assert_eq!(host_target.label, "Target 0");
        assert_eq!(host_target.role, NodeRole::Component);
        let host_adapter = node(&view, &scsi_host_adapter());
        assert_eq!(host_adapter.parent, Some(scsi_target(0)));
        assert_eq!(host_adapter.role, NodeRole::Device);
        assert_eq!(host_adapter.label, "Host Adapter");
        for target in 1..8 {
            for lun in 0..8 {
                let slot = node(&view, &scsi_lun(target, lun));
                assert_eq!(slot.role, NodeRole::Slot);
                let attachment = slot.attachment.as_ref().expect("LUN must be editable");
                assert!(attachment.allow_empty);
                assert!(attachment.current.is_none());
                assert_eq!(
                    attachment
                        .choices
                        .iter()
                        .map(|choice| &choice.id)
                        .collect::<Vec<_>>(),
                    vec![&device_kind(SCSI_DISK), &device_kind(SCSI_CDROM)]
                );
            }
        }
        assert!(!contains(&view, &scsi_lun(0, 0)));

        for (controller, channel, port, port_label, kind, device_label) in [
            (
                0,
                "a",
                Ip12Port::Keyboard,
                "Keyboard Port",
                SGI_KEYBOARD,
                "SGI Keyboard",
            ),
            (
                0,
                "b",
                Ip12Port::Mouse,
                "Mouse Port",
                SGI_MOUSE,
                "SGI Mouse",
            ),
            (
                1,
                "a",
                Ip12Port::SerialA,
                "Serial Port A",
                VT100_TERMINAL,
                "VT100 Terminal",
            ),
            (
                1,
                "b",
                Ip12Port::SerialB,
                "Serial Port B",
                VT100_TERMINAL,
                "VT100 Terminal",
            ),
        ] {
            let id = node_id(&format!("serial.{controller}.channel.{channel}"));
            let channel_node = node(&view, &id);
            assert_eq!(
                channel_node.parent,
                Some(node_id(&format!("serial.{controller}")))
            );
            assert_eq!(channel_node.role, NodeRole::Component);
            assert_eq!(
                channel_node.label,
                format!("Channel {}", channel.to_ascii_uppercase())
            );
            let port_view = node(
                &view,
                &node_id(&format!("serial.{controller}.channel.{channel}.port")),
            );
            assert_eq!(port_view.parent, Some(id.clone()));
            assert_eq!(port_view.role, NodeRole::Endpoint);
            assert_eq!(port_view.label, port_label);
            let attachment = port_view
                .attachment
                .as_ref()
                .expect("port must be editable");
            assert!(attachment.allow_empty);
            assert_eq!(attachment.current, Some(device_kind(kind)));
            assert_eq!(attachment.choices.len(), 1);
            assert_eq!(attachment.choices[0].id, device_kind(kind));
            let device = node(&view, &port_device_node(port));
            assert_eq!(device.parent, Some(port_node(port)));
            assert_eq!(device.role, NodeRole::Device);
            assert_eq!(device.label, device_label);
        }
        assert_eq!(node(&view, &node_id("parallel.0")).role, NodeRole::Endpoint);
        assert_eq!(
            node(&view, &node_id("ethernet.0")).role,
            NodeRole::Component
        );
        assert_eq!(
            node(&view, &node_id("ethernet.0.port.0")).role,
            NodeRole::Endpoint
        );
        assert_eq!(
            node(&view, &node_id("firmware.0")).role,
            NodeRole::Component
        );
        assert_eq!(
            node(&view, &node_id("firmware.0")).properties[0].editor,
            PropertyEditor::Path {
                kind: PathKind::File
            }
        );
        assert!(error(
            &view,
            "ip12.firmware.image-required",
            DiagnosticTarget::Property(property_id(FIRMWARE_PATH))
        ));
    }

    #[test]
    fn topology_has_unique_nodes_in_parent_before_child_order() {
        let view = Ip12Definition.resolve(&Ip12Definition.default_draft());
        let mut seen = BTreeSet::new();
        for item in &view.nodes {
            if let Some(parent) = &item.parent {
                assert!(seen.contains(parent));
            } else {
                assert_eq!(item.id, node_id(MODEL));
            }
            assert!(seen.insert(item.id.clone()));
        }
    }

    #[test]
    fn memory_sockets_follow_the_bank_capacity() {
        let mut draft = Ip12Definition.default_draft();
        set(&mut draft, memory_property('a'), PropertyValue::Integer(8));
        let view = Ip12Definition.resolve(&draft);
        for socket in 0..4 {
            assert_eq!(node(&view, &memory_module('a', socket)).label, "8 MiB SIMM");
        }
        set(&mut draft, memory_property('a'), PropertyValue::Integer(0));
        let view = Ip12Definition.resolve(&draft);
        for socket in 0..4 {
            assert!(contains(&view, &memory_socket('a', socket)));
            assert!(!contains(&view, &memory_module('a', socket)));
        }
        assert!(error(
            &view,
            "ip12.memory.no-installed-bank",
            DiagnosticTarget::Node(node_id("memory"))
        ));
    }

    #[test]
    fn invalid_memory_does_not_satisfy_the_installed_bank_constraint() {
        let mut draft = Ip12Definition.default_draft();
        set(
            &mut draft,
            memory_property('a'),
            PropertyValue::Integer(114_514),
        );
        let view = Ip12Definition.resolve(&draft);
        assert_eq!(
            value(&view, &memory_property('a')),
            &PropertyValue::Integer(114_514)
        );
        assert!(error(
            &view,
            "ip12.property.unsupported-value",
            DiagnosticTarget::Property(memory_property('a'))
        ));
        assert!(error(
            &view,
            "ip12.memory.no-installed-bank",
            DiagnosticTarget::Node(node_id("memory"))
        ));
        assert!(contains(&view, &memory_socket('a', 0)));
        assert!(!contains(&view, &memory_module('a', 0)));
    }

    #[test]
    fn wrong_type_and_missing_memory_remain_visible() {
        let mut draft = Ip12Definition.default_draft();
        set(
            &mut draft,
            memory_property('a'),
            PropertyValue::Text("8".into()),
        );
        let wrong_type = Ip12Definition.resolve(&draft);
        assert_eq!(
            value(&wrong_type, &memory_property('a')),
            &PropertyValue::Text("8".into())
        );
        assert!(error(
            &wrong_type,
            "ip12.property.invalid-type",
            DiagnosticTarget::Property(memory_property('a'))
        ));
        assert!(error(
            &wrong_type,
            "ip12.memory.no-installed-bank",
            DiagnosticTarget::Node(node_id("memory"))
        ));

        draft.properties.remove(&memory_property('a'));
        let missing = Ip12Definition.resolve(&draft);
        assert_eq!(
            value(&missing, &memory_property('a')),
            &PropertyValue::Integer(2)
        );
        assert!(!draft.properties.contains_key(&memory_property('a')));
        assert!(error(
            &missing,
            "ip12.property.missing",
            DiagnosticTarget::Property(memory_property('a'))
        ));
    }

    #[test]
    fn missing_memory_property_uses_fallback_for_resolved_constraints() {
        let mut draft = Ip12Definition.default_draft();
        let property = memory_property('a');
        draft.properties.remove(&property);

        let view = Ip12Definition.resolve(&draft);

        assert!(!draft.properties.contains_key(&property));
        assert_eq!(value(&view, &property), &PropertyValue::Integer(2));
        assert!(error(
            &view,
            "ip12.property.missing",
            DiagnosticTarget::Property(property)
        ));
        for socket in 0..4 {
            assert_eq!(node(&view, &memory_module('a', socket)).label, "2 MiB SIMM");
        }
        assert!(!error(
            &view,
            "ip12.memory.no-installed-bank",
            DiagnosticTarget::Node(node_id("memory"))
        ));
    }

    #[test]
    fn fpu_backend_choices_and_invalid_values_stay_on_the_r3010() {
        let mut draft = Ip12Definition.default_draft();
        let fpu = node(&Ip12Definition.resolve(&draft), &node_id("cpu.0.fpu.0")).clone();
        assert!(matches!(
            fpu.properties[0].editor,
            PropertyEditor::Choice { .. }
        ));
        for backend in ["softfloat", "native"] {
            set(
                &mut draft,
                property_id(FPU_BACKEND),
                PropertyValue::Text(backend.into()),
            );
            let view = Ip12Definition.resolve(&draft);
            assert_eq!(
                value(&view, &property_id(FPU_BACKEND)),
                &PropertyValue::Text(backend.into())
            );
            assert!(!error(
                &view,
                "ip12.property.unsupported-value",
                DiagnosticTarget::Property(property_id(FPU_BACKEND))
            ));
        }
        set(
            &mut draft,
            property_id(FPU_BACKEND),
            PropertyValue::Text("unknown".into()),
        );
        let unknown = Ip12Definition.resolve(&draft);
        assert_eq!(
            value(&unknown, &property_id(FPU_BACKEND)),
            &PropertyValue::Text("unknown".into())
        );
        assert!(error(
            &unknown,
            "ip12.property.unsupported-value",
            DiagnosticTarget::Property(property_id(FPU_BACKEND))
        ));
        set(
            &mut draft,
            property_id(FPU_BACKEND),
            PropertyValue::Integer(1),
        );
        assert!(error(
            &Ip12Definition.resolve(&draft),
            "ip12.property.invalid-type",
            DiagnosticTarget::Property(property_id(FPU_BACKEND))
        ));
        draft.properties.remove(&property_id(FPU_BACKEND));
        let missing = Ip12Definition.resolve(&draft);
        assert_eq!(
            value(&missing, &property_id(FPU_BACKEND)),
            &PropertyValue::Text("softfloat".into())
        );
        assert!(error(
            &missing,
            "ip12.property.missing",
            DiagnosticTarget::Property(property_id(FPU_BACKEND))
        ));
        assert!(contains(&missing, &node_id("cpu.0.fpu.0")));
    }

    #[test]
    fn graphics_device_and_video_endpoint_follow_the_attachment() {
        let mut draft = Ip12Definition.default_draft();
        attach(&mut draft, node_id(GRAPHICS_SLOT), None);
        let detached = Ip12Definition.resolve(&draft);
        assert!(!contains(&detached, &node_id("gio.0.slot.graphics.device")));
        assert!(!contains(
            &detached,
            &node_id("gio.0.slot.graphics.device.video.0")
        ));
        assert!(
            node(&detached, &node_id(GRAPHICS_SLOT))
                .attachment
                .as_ref()
                .expect("slot must be editable")
                .current
                .is_none()
        );

        attach(&mut draft, node_id(GRAPHICS_SLOT), Some(LG1));
        assert!(contains(
            &Ip12Definition.resolve(&draft),
            &node_id("gio.0.slot.graphics.device.video.0")
        ));

        attach(&mut draft, node_id(GRAPHICS_SLOT), Some(SCSI_DISK));
        let invalid = Ip12Definition.resolve(&draft);
        assert_eq!(
            node(&invalid, &node_id(GRAPHICS_SLOT))
                .attachment
                .as_ref()
                .expect("slot must be editable")
                .current,
            Some(device_kind(SCSI_DISK))
        );
        assert!(
            node(&invalid, &node_id("gio.0.slot.graphics.device"))
                .label
                .contains(SCSI_DISK)
        );
        assert!(!contains(
            &invalid,
            &node_id("gio.0.slot.graphics.device.video.0")
        ));
        assert!(error(
            &invalid,
            "ip12.attachment.unsupported-device",
            DiagnosticTarget::Node(node_id(GRAPHICS_SLOT))
        ));
    }

    #[test]
    fn each_port_keeps_its_interface_when_its_peripheral_is_detached() {
        for port in [
            Ip12Port::Keyboard,
            Ip12Port::Mouse,
            Ip12Port::SerialA,
            Ip12Port::SerialB,
        ] {
            let mut draft = valid_draft();
            attach(&mut draft, port_node(port), None);
            let view = Ip12Definition.resolve(&draft);
            let port_view = node(&view, &port_node(port));
            assert!(
                port_view
                    .attachment
                    .as_ref()
                    .expect("port must stay editable")
                    .current
                    .is_none()
            );
            assert!(!contains(&view, &port_device_node(port)));
            let plan = Ip12Definition
                .compile(&draft)
                .expect("an empty port is valid");
            assert!(
                plan.port_attachments()
                    .iter()
                    .all(|attachment| attachment.port() != port)
            );
        }
    }

    #[test]
    fn incompatible_port_peripherals_stay_visible_and_fail_compilation() {
        for (port, device) in [
            (Ip12Port::Keyboard, VT100_TERMINAL),
            (Ip12Port::SerialA, SGI_MOUSE),
        ] {
            let mut draft = valid_draft();
            attach(&mut draft, port_node(port), Some(device));
            let view = Ip12Definition.resolve(&draft);
            assert_eq!(
                node(&view, &port_node(port))
                    .attachment
                    .as_ref()
                    .expect("port must be editable")
                    .current,
                Some(device_kind(device))
            );
            assert!(node(&view, &port_device_node(port)).label.contains(device));
            assert!(error(
                &view,
                "ip12.attachment.unsupported-device",
                DiagnosticTarget::Node(port_node(port))
            ));
            assert_compile_matches_resolution(&draft);
        }
    }

    #[test]
    fn scsi_host_target_cannot_accept_a_device() {
        let mut draft = Ip12Definition.default_draft();
        let host_lun = scsi_lun(0, 0);
        attach(&mut draft, host_lun.clone(), Some(SCSI_DISK));
        let view = Ip12Definition.resolve(&draft);
        assert!(contains(&view, &scsi_target(0)));
        assert!(!contains(&view, &host_lun));
        assert!(error(
            &view,
            "ip12.attachment.unknown-slot",
            DiagnosticTarget::Node(host_lun)
        ));
    }

    #[test]
    fn multiple_scsi_devices_have_independent_nodes_and_media() {
        let mut draft = Ip12Definition.default_draft();
        let devices = [
            (1, 0, SCSI_DISK, "one.img"),
            (2, 0, SCSI_DISK, "two.img"),
            (3, 5, SCSI_DISK, "three.img"),
            (4, 0, SCSI_CDROM, "install.iso"),
        ];
        for (target, lun, kind, path) in devices {
            attach(&mut draft, scsi_lun(target, lun), Some(kind));
            set(
                &mut draft,
                scsi_medium(target, lun),
                PropertyValue::Text(path.into()),
            );
        }
        let view = Ip12Definition.resolve(&draft);
        for (target, lun, kind, path) in devices {
            let slot = node(&view, &scsi_lun(target, lun));
            assert_eq!(
                slot.attachment
                    .as_ref()
                    .expect("LUN must be editable")
                    .current,
                Some(device_kind(kind))
            );
            let device = node(&view, &scsi_device(target, lun));
            assert_eq!(device.parent, Some(scsi_lun(target, lun)));
            assert_eq!(
                value(&view, &scsi_medium(target, lun)),
                &PropertyValue::Text(path.into())
            );
            assert_eq!(device.properties.len(), 1);
            assert_eq!(
                device.properties[0].editor,
                PropertyEditor::Path {
                    kind: PathKind::File
                }
            );
            assert_eq!(
                device.properties[0].label,
                if kind == SCSI_DISK {
                    "Disk image"
                } else {
                    "Initial medium"
                }
            );
        }
        assert!(
            !view
                .diagnostics
                .iter()
                .any(|item| item.code == "ip12.scsi.medium-required")
        );
    }

    #[test]
    fn medium_path_is_required_only_while_a_scsi_device_is_attached() {
        let slot = scsi_lun(2, 3);
        let medium = scsi_medium(2, 3);
        let target = DiagnosticTarget::Property(medium.clone());

        let mut disk = valid_draft();
        attach(&mut disk, slot.clone(), Some(SCSI_DISK));
        for missing in [None, Some(""), Some("   ")] {
            if let Some(path) = missing {
                set(&mut disk, medium.clone(), PropertyValue::Text(path.into()));
            }
            let view = Ip12Definition.resolve(&disk);
            assert_eq!(
                targeted_codes(&view, &target),
                ["ip12.scsi.medium-required"]
            );
            assert_eq!(
                view.diagnostics
                    .iter()
                    .find(|diagnostic| diagnostic.target == target)
                    .map(|diagnostic| diagnostic.message.as_str()),
                Some("Select a disk image.")
            );
            assert!(Ip12Definition.compile(&disk).is_err());
        }

        let mut cdrom = valid_draft();
        attach(&mut cdrom, slot.clone(), Some(SCSI_CDROM));
        for missing in [None, Some(""), Some("   ")] {
            if let Some(path) = missing {
                set(&mut cdrom, medium.clone(), PropertyValue::Text(path.into()));
            }
            let view = Ip12Definition.resolve(&cdrom);
            assert!(targeted_codes(&view, &target).is_empty());
            let plan = Ip12Definition
                .compile(&cdrom)
                .expect("an empty CD-ROM drive is complete");
            assert_eq!(plan.scsi().len(), 1);
            assert_eq!(plan.scsi()[0].device, ScsiDevice::Cdrom);
            assert_eq!(plan.scsi()[0].medium(), None);
            assert_eq!(plan.resources().len(), 1);
        }

        for kind in [SCSI_DISK, SCSI_CDROM] {
            let mut draft = valid_draft();
            attach(&mut draft, slot.clone(), Some(kind));
            set(&mut draft, medium.clone(), PropertyValue::Integer(3));
            let wrong_type = Ip12Definition.resolve(&draft);
            assert_eq!(value(&wrong_type, &medium), &PropertyValue::Integer(3));
            assert_eq!(
                targeted_codes(&wrong_type, &target),
                ["ip12.property.invalid-type"]
            );
        }

        let mut draft = Ip12Definition.default_draft();
        attach(&mut draft, slot.clone(), Some(SCSI_DISK));
        set(
            &mut draft,
            medium.clone(),
            PropertyValue::Text("disk.img".into()),
        );
        assert!(!error(
            &Ip12Definition.resolve(&draft),
            "ip12.scsi.medium-required",
            DiagnosticTarget::Property(medium.clone())
        ));
        attach(&mut draft, slot.clone(), None);
        let detached = Ip12Definition.resolve(&draft);
        assert_eq!(
            draft.properties.get(&medium),
            Some(&PropertyValue::Text("disk.img".into()))
        );
        assert!(!contains(&detached, &scsi_device(2, 3)));
        assert!(
            !detached
                .nodes
                .iter()
                .flat_map(|node| &node.properties)
                .any(|property| property.id == medium)
        );
        assert!(
            !detached
                .diagnostics
                .iter()
                .any(|item| item.target == DiagnosticTarget::Property(medium.clone()))
        );
        attach(&mut draft, slot, Some(SCSI_CDROM));
        assert_eq!(
            value(&Ip12Definition.resolve(&draft), &medium),
            &PropertyValue::Text("disk.img".into())
        );
    }

    #[test]
    fn bad_drafts_keep_the_topology_and_report_each_problem() {
        let mut draft = Ip12Definition.default_draft();
        draft.model = MachineModelId("indy".into());
        let unknown_property = property_id("unrecognized.setting");
        let unknown_slot = node_id("gio.0.slot.99");
        set(
            &mut draft,
            unknown_property.clone(),
            PropertyValue::Bool(true),
        );
        attach(&mut draft, unknown_slot.clone(), Some(LG1));
        attach(&mut draft, scsi_lun(5, 6), Some("unknown.device"));
        let view = Ip12Definition.resolve(&draft);
        assert!(contains(&view, &node_id(MODEL)));
        assert!(contains(&view, &scsi_lun(5, 6)));
        assert!(
            node(&view, &scsi_device(5, 6))
                .label
                .contains("unknown.device")
        );
        assert_eq!(
            node(&view, &scsi_lun(5, 6))
                .attachment
                .as_ref()
                .expect("slot must be editable")
                .current,
            Some(DeviceKindId("unknown.device".into()))
        );
        assert!(error(
            &view,
            "ip12.model.mismatch",
            DiagnosticTarget::Global
        ));
        assert!(error(
            &view,
            "ip12.property.unknown",
            DiagnosticTarget::Property(unknown_property)
        ));
        assert!(error(
            &view,
            "ip12.attachment.unknown-slot",
            DiagnosticTarget::Node(unknown_slot)
        ));
        assert!(error(
            &view,
            "ip12.attachment.unsupported-device",
            DiagnosticTarget::Node(scsi_lun(5, 6))
        ));
    }

    #[test]
    fn firmware_requires_a_text_path_without_opening_it() {
        let mut draft = Ip12Definition.default_draft();
        let property = property_id(FIRMWARE_PATH);
        set(
            &mut draft,
            property.clone(),
            PropertyValue::Text("does-not-exist.prom".into()),
        );
        let supplied = Ip12Definition.resolve(&draft);
        assert!(!error(
            &supplied,
            "ip12.firmware.image-required",
            DiagnosticTarget::Property(property.clone())
        ));
        set(&mut draft, property.clone(), PropertyValue::Bool(true));
        let wrong_type = Ip12Definition.resolve(&draft);
        assert_eq!(value(&wrong_type, &property), &PropertyValue::Bool(true));
        assert!(error(
            &wrong_type,
            "ip12.property.invalid-type",
            DiagnosticTarget::Property(property.clone())
        ));
        assert_eq!(
            targeted_codes(&wrong_type, &DiagnosticTarget::Property(property.clone())),
            ["ip12.property.invalid-type"]
        );
        draft.properties.remove(&property);
        let missing = Ip12Definition.resolve(&draft);
        assert_eq!(
            value(&missing, &property),
            &PropertyValue::Text(String::new())
        );
        let target = DiagnosticTarget::Property(property);
        assert_eq!(
            targeted_codes(&missing, &target),
            ["ip12.firmware.image-required"]
        );
        assert_eq!(
            missing
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.target == target)
                .map(|diagnostic| diagnostic.message.as_str()),
            Some("Select a PROM image.")
        );
    }
}
