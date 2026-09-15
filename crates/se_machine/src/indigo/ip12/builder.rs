//! Assembly of a validated IP12 plan from owned machine capabilities.

use std::error::Error;
use std::fmt;

use se_device::gio::{GioAttachError, GioBus, GioSlot};
use se_device::lg1::Lg1;
use se_device::scsi::{ScsiAttachError, ScsiBus, ScsiStorageSizeError, ScsiTarget};
use se_device::scsi_cdrom::ScsiCdrom;
use se_device::scsi_disk::ScsiDisk;
use se_device::sgi_keyboard::SgiKeyboard;
use se_device::sgi_mouse::SgiMouse;

use super::plan::{GioDevice, Ip12Peripheral, Ip12Port, PreparedIp12Build, ScsiDevice};
use super::{Ip12, Ip12Error};

/// An error encountered while assembling validated IP12 hardware.
///
/// Unlike a configuration compile error, this error concerns prepared
/// capabilities or physical device construction.
#[derive(Debug)]
pub enum Ip12AssemblyError {
    /// A selected GIO device could not be installed in its slot.
    GioAttachment {
        /// The physical slot selected by the build plan.
        slot: GioSlot,
        /// The bus attachment failure.
        source: GioAttachError,
    },
    /// A SCSI medium has a capacity unsupported by its device model.
    InvalidScsiMedium {
        /// The selected target ID.
        target: u8,
        /// The selected LUN.
        lun: u8,
        /// The selected device model.
        device: ScsiDevice,
        /// The rejected medium capacity.
        source: ScsiStorageSizeError,
    },
    /// A SCSI device could not be installed at its planned address.
    ScsiAttachment {
        /// The selected target ID.
        target: u8,
        /// The selected LUN.
        lun: u8,
        /// The selected device model.
        device: ScsiDevice,
        /// The bus attachment failure.
        source: ScsiAttachError,
    },
    /// Fixed board construction rejected the supplied machine inputs.
    BoardConstruction(Ip12Error),
}

impl fmt::Display for Ip12AssemblyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GioAttachment { slot, source } => {
                write!(
                    formatter,
                    "cannot attach IP12 GIO device at {slot:?}: {source}"
                )
            }
            Self::InvalidScsiMedium {
                target,
                lun,
                device,
                source,
            } => write!(
                formatter,
                "invalid IP12 SCSI {device:?} medium at target {target} LUN {lun}: {source}"
            ),
            Self::ScsiAttachment {
                target,
                lun,
                device,
                source,
            } => write!(
                formatter,
                "cannot attach IP12 SCSI {device:?} at target {target} LUN {lun}: {source}"
            ),
            Self::BoardConstruction(source) => {
                write!(formatter, "cannot construct the IP12 board: {source}")
            }
        }
    }
}

impl Error for Ip12AssemblyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::GioAttachment { source, .. } => Some(source),
            Self::InvalidScsiMedium { source, .. } => Some(source),
            Self::ScsiAttachment { source, .. } => Some(source),
            Self::BoardConstruction(source) => Some(source),
        }
    }
}

/// Assembles an IP12 from a plan bound to its prepared capabilities.
///
/// Assembly consumes each capability and leaves host paths outside the machine.
///
/// # Errors
///
/// Returns [`Ip12AssemblyError`] when a prepared image or device cannot form
/// valid IP12 hardware.
pub fn build(prepared: PreparedIp12Build) -> Result<Ip12, Ip12AssemblyError> {
    let (plan, mut resources) = prepared.into_parts();
    let raw_prom = resources.take_bytes(plan.firmware());

    let mut gio = GioBus::new();
    for attachment in plan.gio() {
        let device = match attachment.device {
            GioDevice::Lg1 => Box::new(Lg1::new()),
        };
        gio.attach(attachment.slot, device)
            .map_err(|source| Ip12AssemblyError::GioAttachment {
                slot: attachment.slot,
                source,
            })?;
    }

    let mut scsi = ScsiBus::new();
    for attachment in plan.scsi() {
        let storage = resources.take_storage(&attachment.medium);
        let bytes = storage.size_bytes();
        let target: Box<dyn ScsiTarget> = match attachment.device {
            ScsiDevice::Disk => Box::new(ScsiDisk::try_new(bytes).map_err(|source| {
                Ip12AssemblyError::InvalidScsiMedium {
                    target: attachment.target,
                    lun: attachment.lun,
                    device: attachment.device,
                    source,
                }
            })?),
            ScsiDevice::Cdrom => Box::new(ScsiCdrom::try_new(bytes).map_err(|source| {
                Ip12AssemblyError::InvalidScsiMedium {
                    target: attachment.target,
                    lun: attachment.lun,
                    device: attachment.device,
                    source,
                }
            })?),
        };
        scsi.attach(attachment.target, attachment.lun, target, storage)
            .map_err(|source| Ip12AssemblyError::ScsiAttachment {
                target: attachment.target,
                lun: attachment.lun,
                device: attachment.device,
                source,
            })?;
    }

    let mut keyboard = None;
    let mut mouse = None;
    for attachment in plan.port_attachments() {
        match attachment.peripheral() {
            Ip12Peripheral::SgiKeyboard => {
                assert_eq!(attachment.port(), Ip12Port::Keyboard);
                assert!(
                    keyboard.replace(SgiKeyboard::new()).is_none(),
                    "an IP12 build plan cannot attach the keyboard port twice"
                );
            }
            Ip12Peripheral::SgiMouse => {
                assert_eq!(attachment.port(), Ip12Port::Mouse);
                assert!(
                    mouse.replace(SgiMouse::new()).is_none(),
                    "an IP12 build plan cannot attach the mouse port twice"
                );
            }
            Ip12Peripheral::Vt100Terminal => {
                assert!(matches!(
                    attachment.port(),
                    Ip12Port::SerialA | Ip12Port::SerialB
                ));
            }
        }
    }

    let machine = Ip12::new_with_buses(
        raw_prom,
        plan.floating_point_backend(),
        *plan.memory(),
        gio,
        scsi,
        keyboard,
        mouse,
    )
    .map_err(Ip12AssemblyError::BoardConstruction)?;
    assert!(
        resources.is_empty(),
        "IP12 assembly must consume every prepared resource"
    );
    Ok(machine)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::convert::Infallible;
    use std::io;

    use se_config::definition::MachineDefinition;
    use se_config::draft::{Edit, MachineDraft};
    use se_config::id::{DeviceKindId, NodeId, PropertyId};
    use se_config::value::PropertyValue;
    use se_core::storage::StorageMedium;

    use super::super::definition::Ip12Definition;
    use super::super::plan::{Ip12BuildPlan, Ip12Port, PreparedIp12Build, ScsiDevice};
    use super::super::{Ip12, Ip12Error, Ip12SnapshotError, PROM_BYTES};
    use super::{Ip12AssemblyError, build};
    use crate::endpoint::EndpointKind;
    use crate::input::{KeyboardKey, MachineInput, MachineInputPayload};
    use crate::machine::{Machine, MachineInputError};
    use crate::resource::{PreparedResource, ResourceKind};

    struct MemoryStorage {
        bytes: Vec<u8>,
    }

    impl StorageMedium for MemoryStorage {
        fn size_bytes(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
            let start = usize::try_from(offset).map_err(|_| io::Error::other("bad offset"))?;
            let end = start
                .checked_add(buffer.len())
                .ok_or_else(|| io::Error::other("range overflow"))?;
            let source = self
                .bytes
                .get(start..end)
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short storage"))?;
            buffer.copy_from_slice(source);
            Ok(())
        }

        fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
            let start = usize::try_from(offset).map_err(|_| io::Error::other("bad offset"))?;
            let end = start
                .checked_add(data.len())
                .ok_or_else(|| io::Error::other("range overflow"))?;
            let destination = self
                .bytes
                .get_mut(start..end)
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short storage"))?;
            destination.copy_from_slice(data);
            Ok(())
        }
    }

    fn draft(devices: &[(u8, u8, ScsiDevice)], graphics: bool) -> MachineDraft {
        let mut draft = Ip12Definition.default_draft();
        draft.apply(Edit::SetProperty {
            property: PropertyId("firmware.0.image-path".into()),
            value: PropertyValue::Text("unused.prom".into()),
        });
        if !graphics {
            draft.apply(Edit::SetAttachment {
                slot: NodeId("gio.0.slot.graphics".into()),
                device: None,
            });
        }
        for &(target, lun, device) in devices {
            let kind = match device {
                ScsiDevice::Disk => "scsi.disk",
                ScsiDevice::Cdrom => "scsi.cdrom",
            };
            draft.apply(Edit::SetAttachment {
                slot: NodeId(format!("scsi.0.target.{target}.lun.{lun}")),
                device: Some(DeviceKindId(kind.into())),
            });
            draft.apply(Edit::SetProperty {
                property: PropertyId(format!("scsi.0.target.{target}.lun.{lun}.medium-path")),
                value: PropertyValue::Text(format!("unused-{target}-{lun}.img")),
            });
        }
        draft
    }

    fn prepare(
        plan: Ip12BuildPlan,
        firmware_bytes: usize,
        capacities: &BTreeMap<(u8, u8), usize>,
    ) -> PreparedIp12Build {
        let medium_addresses: BTreeMap<_, _> = plan
            .scsi()
            .iter()
            .map(|attachment| {
                (
                    attachment.medium.clone(),
                    (attachment.target, attachment.lun),
                )
            })
            .collect();
        plan.prepare_with(|id, requirement| {
            Ok::<_, Infallible>(match requirement.kind {
                ResourceKind::Bytes => PreparedResource::Bytes(vec![0; firmware_bytes]),
                ResourceKind::Storage { access } => {
                    let address = medium_addresses
                        .get(id)
                        .expect("every medium role belongs to an attachment");
                    let capacity = capacities.get(address).copied().unwrap_or(2048);
                    PreparedResource::Storage {
                        access,
                        medium: Box::new(MemoryStorage {
                            bytes: vec![0; capacity],
                        }),
                    }
                }
            })
        })
        .expect("memory capabilities must prepare")
    }

    fn detach_port(draft: &mut MachineDraft, port: Ip12Port) {
        let slot = match port {
            Ip12Port::Keyboard => "serial.0.channel.a.port",
            Ip12Port::Mouse => "serial.0.channel.b.port",
            Ip12Port::SerialA => "serial.1.channel.a.port",
            Ip12Port::SerialB => "serial.1.channel.b.port",
        };
        draft.apply(Edit::SetAttachment {
            slot: NodeId(String::from(slot)),
            device: None,
        });
    }

    fn build_draft(configuration: &MachineDraft) -> Ip12 {
        let plan = Ip12Definition
            .compile(configuration)
            .expect("the draft is valid");
        build(prepare(plan, PROM_BYTES, &BTreeMap::new())).expect("the machine assembles")
    }

    #[test]
    fn firmware_capability_constructs_an_ip12() {
        let plan = Ip12Definition
            .compile(&draft(&[], true))
            .expect("the draft is valid");
        let prepared = prepare(plan, PROM_BYTES, &BTreeMap::new());
        let machine = build(prepared).expect("the PROM bytes construct a board");
        assert!(machine.video_output().is_some());
    }

    #[test]
    fn an_empty_graphics_slot_stays_empty_after_assembly() {
        let plan = Ip12Definition
            .compile(&draft(&[], false))
            .expect("the draft is valid");
        let prepared = prepare(plan, PROM_BYTES, &BTreeMap::new());
        let machine = build(prepared).expect("an empty graphics slot is valid");
        assert_eq!(machine.video_output(), None);
    }

    #[test]
    fn optional_keyboard_and_mouse_control_hardware_endpoints() {
        for (port, kind) in [
            (Ip12Port::Keyboard, EndpointKind::Keyboard),
            (Ip12Port::Mouse, EndpointKind::Pointer),
        ] {
            let mut configuration = draft(&[], true);
            detach_port(&mut configuration, port);
            let machine = build_draft(&configuration);
            assert!(
                machine
                    .endpoint_catalog()
                    .endpoints()
                    .iter()
                    .all(|endpoint| endpoint.kind() != kind)
            );
            let payload = match port {
                Ip12Port::Keyboard => MachineInputPayload::Keyboard {
                    key: KeyboardKey::Letter(b'A'),
                    pressed: true,
                },
                Ip12Port::Mouse => MachineInputPayload::PointerMotion {
                    delta_x: 1,
                    delta_y: 1,
                },
                Ip12Port::SerialA | Ip12Port::SerialB => unreachable!(),
            };
            let input = MachineInput::new(port.endpoint_key(), payload);
            let mut machine = Machine::IndigoIp12(machine);
            assert!(matches!(
                machine.try_receive_input(&input),
                Err(MachineInputError::UnknownEndpoint)
            ));
        }
    }

    #[test]
    fn serial_interfaces_exist_without_vt100_peripherals() {
        let mut configuration = draft(&[], true);
        detach_port(&mut configuration, Ip12Port::SerialA);
        detach_port(&mut configuration, Ip12Port::SerialB);
        let machine = build_draft(&configuration);
        assert_eq!(
            machine
                .endpoint_catalog()
                .endpoints()
                .iter()
                .filter(|endpoint| endpoint.kind() == EndpointKind::Serial)
                .map(|endpoint| endpoint.key().as_str())
                .collect::<Vec<_>>(),
            ["serial.external.a", "serial.external.b"]
        );
    }

    #[test]
    fn snapshots_reject_keyboard_and_mouse_presence_mismatches() {
        for port in [Ip12Port::Keyboard, Ip12Port::Mouse] {
            let attached_configuration = draft(&[], true);
            let attached = build_draft(&attached_configuration);
            let attached_snapshot = attached.snapshot().unwrap();

            let mut detached_configuration = attached_configuration.clone();
            detach_port(&mut detached_configuration, port);
            let detached = build_draft(&detached_configuration);
            let detached_snapshot = detached.snapshot().unwrap();

            let mut detached_target = build_draft(&detached_configuration);
            assert!(matches!(
                detached_target.restore_snapshot(attached_snapshot),
                Err(Ip12SnapshotError::PortAttachmentMismatch {
                    port: failed_port,
                    snapshot_attached: true,
                    machine_attached: false,
                }) if failed_port == port
            ));

            let mut attached_target = build_draft(&attached_configuration);
            assert!(matches!(
                attached_target.restore_snapshot(detached_snapshot),
                Err(Ip12SnapshotError::PortAttachmentMismatch {
                    port: failed_port,
                    snapshot_attached: false,
                    machine_attached: true,
                }) if failed_port == port
            ));
        }
    }

    #[test]
    fn prepared_build_keeps_the_exact_plan_when_resource_paths_change() {
        let medium = PropertyId("scsi.0.target.1.lun.0.medium-path".into());
        let mut draft_a = draft(&[(1, 0, ScsiDevice::Disk)], true);
        draft_a.apply(Edit::SetProperty {
            property: medium.clone(),
            value: PropertyValue::Text("old.img".into()),
        });
        let mut draft_b = draft_a.clone();
        draft_b.apply(Edit::SetProperty {
            property: medium,
            value: PropertyValue::Text("new.img".into()),
        });

        let plan_a = Ip12Definition.compile(&draft_a).expect("old path is valid");
        let plan_b = Ip12Definition.compile(&draft_b).expect("new path is valid");
        let role = plan_a.scsi()[0].medium.clone();
        assert_eq!(plan_b.scsi()[0].medium, role);
        assert_eq!(
            plan_a.resources().get(&role).map(|item| item.kind),
            plan_b.resources().get(&role).map(|item| item.kind)
        );

        let prepared_a = prepare(plan_a, PROM_BYTES, &BTreeMap::new());
        let (bound_plan, mut capabilities) = prepared_a.into_parts();
        assert_eq!(
            bound_plan.resources().get(&role).unwrap().path,
            std::path::Path::new("old.img")
        );
        assert_eq!(
            plan_b.resources().get(&role).unwrap().path,
            std::path::Path::new("new.img")
        );
        assert_eq!(capabilities.take_storage(&role).size_bytes(), 2048);
    }

    #[test]
    fn multiple_scsi_attachments_reach_the_machine_snapshot_topology() {
        let devices = [
            (1, 0, ScsiDevice::Disk),
            (2, 0, ScsiDevice::Disk),
            (3, 5, ScsiDevice::Disk),
            (4, 0, ScsiDevice::Cdrom),
        ];
        let configuration = draft(&devices, true);
        let plan_a = Ip12Definition
            .compile(&configuration)
            .expect("valid topology");
        assert_eq!(plan_a.scsi().len(), devices.len());
        let prepared_a = prepare(plan_a, PROM_BYTES, &BTreeMap::new());
        let machine_a = build(prepared_a).expect("all devices attach");

        let plan_b = Ip12Definition
            .compile(&configuration)
            .expect("same valid topology");
        let prepared_b = prepare(plan_b, PROM_BYTES, &BTreeMap::new());
        let mut machine_b = build(prepared_b).expect("same devices attach");
        let snapshot = machine_a.snapshot().expect("the topology is restorable");
        machine_b
            .restore_snapshot(snapshot.clone())
            .expect("identical SCSI topology accepts the snapshot");

        let without_third_disk = [devices[0], devices[1], devices[3]];
        let plan_c = Ip12Definition
            .compile(&draft(&without_third_disk, true))
            .expect("smaller topology is valid");
        let prepared_c = prepare(plan_c, PROM_BYTES, &BTreeMap::new());
        let mut machine_c = build(prepared_c).expect("smaller topology assembles");
        assert!(matches!(
            machine_c.restore_snapshot(snapshot),
            Err(Ip12SnapshotError::Scsi(_))
        ));
    }

    #[test]
    fn invalid_disk_capacity_is_an_assembly_error_with_address() {
        let plan = Ip12Definition
            .compile(&draft(&[(3, 5, ScsiDevice::Disk)], true))
            .expect("disk capacity is not a draft property");
        let prepared = prepare(plan, PROM_BYTES, &BTreeMap::from([((3, 5), 123)]));
        let error = match build(prepared) {
            Ok(_) => panic!("an invalid disk capacity must fail assembly"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            Ip12AssemblyError::InvalidScsiMedium {
                target: 3,
                lun: 5,
                device: ScsiDevice::Disk,
                ..
            }
        ));
    }

    #[test]
    fn invalid_cdrom_capacity_is_an_assembly_error_with_address() {
        let plan = Ip12Definition
            .compile(&draft(&[(4, 0, ScsiDevice::Cdrom)], true))
            .expect("CD-ROM capacity is not a draft property");
        let prepared = prepare(plan, PROM_BYTES, &BTreeMap::from([((4, 0), 512)]));
        let error = match build(prepared) {
            Ok(_) => panic!("an invalid CD-ROM capacity must fail assembly"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            Ip12AssemblyError::InvalidScsiMedium {
                target: 4,
                lun: 0,
                device: ScsiDevice::Cdrom,
                ..
            }
        ));
    }

    #[test]
    fn invalid_prom_capacity_is_a_board_assembly_error() {
        let plan = Ip12Definition
            .compile(&draft(&[], true))
            .expect("the path is semantically valid");
        let prepared = prepare(plan, 1, &BTreeMap::new());
        let error = match build(prepared) {
            Ok(_) => panic!("a one-byte PROM cannot construct an IP12"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            Ip12AssemblyError::BoardConstruction(Ip12Error::InvalidPromSize {
                expected: PROM_BYTES,
                actual: 1
            })
        ));
    }
}
