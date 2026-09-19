//! Validated, strongly typed construction requirements for an Indigo IP12.

use std::error::Error;
use std::fmt;

use se_config::diagnostic::Diagnostic;
use se_device::gio::GioSlot;
use se_float::backend::Backend;

use super::Ip12MemoryConfiguration;
use crate::endpoint::EndpointKey;
use crate::resource::{
    PrepareResourcesError, PreparedResource, PreparedResources, ResourceId, ResourceRequirement,
    ResourceRequirements,
};

/// An IP12 configuration ready for later resource preparation and assembly.
///
/// Only successful [`Ip12Definition::compile`](super::definition::Ip12Definition::compile)
/// creates this value. It contains no open resources or machine instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip12BuildPlan {
    floating_point_backend: Backend,
    memory: Ip12MemoryConfiguration,
    firmware: ResourceId,
    gio: Vec<GioAttachment>,
    scsi: Vec<ScsiAttachment>,
    port_attachments: Vec<Ip12PortAttachment>,
    resources: ResourceRequirements,
}

impl Ip12BuildPlan {
    pub(super) fn new(
        floating_point_backend: Backend,
        memory: Ip12MemoryConfiguration,
        firmware: ResourceId,
        gio: Vec<GioAttachment>,
        scsi: Vec<ScsiAttachment>,
        port_attachments: Vec<Ip12PortAttachment>,
        resources: ResourceRequirements,
    ) -> Self {
        assert!(
            port_attachments
                .windows(2)
                .all(|pair| pair[0].port < pair[1].port),
            "IP12 port attachments must be unique and canonically ordered"
        );
        Self {
            floating_point_backend,
            memory,
            firmware,
            gio,
            scsi,
            port_attachments,
            resources,
        }
    }

    /// Prepares every resource and binds the resulting capabilities to this plan.
    ///
    /// The provider receives each resource role and its host requirement. A
    /// successful result owns both this exact plan and all capabilities needed
    /// to assemble it.
    ///
    /// # Errors
    ///
    /// Returns [`PrepareResourcesError`] if a provider fails or supplies an
    /// incompatible resource kind or access mode.
    pub fn prepare_with<E>(
        self,
        prepare: impl FnMut(&ResourceId, &ResourceRequirement) -> Result<PreparedResource, E>,
    ) -> Result<PreparedIp12Build, PrepareResourcesError<E>> {
        let resources = self.resources.prepare_with(prepare)?;
        Ok(PreparedIp12Build {
            plan: self,
            resources,
        })
    }

    /// Returns the selected floating-point implementation.
    #[must_use]
    pub fn floating_point_backend(&self) -> Backend {
        self.floating_point_backend
    }

    /// Returns installed SIMM capacities for banks A, B, and C.
    #[must_use]
    pub fn memory(&self) -> &Ip12MemoryConfiguration {
        &self.memory
    }

    /// Returns the logical resource role for the PROM image.
    #[must_use]
    pub fn firmware(&self) -> &ResourceId {
        &self.firmware
    }

    /// Returns GIO devices in physical slot order.
    #[must_use]
    pub fn gio(&self) -> &[GioAttachment] {
        &self.gio
    }

    /// Returns SCSI devices in target-then-LUN order.
    #[must_use]
    pub fn scsi(&self) -> &[ScsiAttachment] {
        &self.scsi
    }

    /// Returns attached port peripherals in canonical physical port order.
    #[must_use]
    pub fn port_attachments(&self) -> &[Ip12PortAttachment] {
        &self.port_attachments
    }

    /// Returns host resources required for assembly.
    #[must_use]
    pub fn resources(&self) -> &ResourceRequirements {
        &self.resources
    }
}

/// An IP12 plan and the capabilities prepared for that exact plan.
///
/// The pair cannot be assembled with a different plan or separated by callers.
pub struct PreparedIp12Build {
    plan: Ip12BuildPlan,
    resources: PreparedResources,
}

impl PreparedIp12Build {
    pub(super) fn into_parts(self) -> (Ip12BuildPlan, PreparedResources) {
        (self.plan, self.resources)
    }
}

/// One device selected for a GIO slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GioAttachment {
    /// Physical GIO slot.
    pub slot: GioSlot,
    /// Device to install.
    pub device: GioDevice,
}

/// A supported GIO device selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GioDevice {
    /// LG1 entry graphics.
    Lg1,
}

/// One independently addressed SCSI device and its cold-start medium.
///
/// The medium role is private so that only `ScsiAttachment::disk` and
/// `ScsiAttachment::cdrom` can pair a device with it. A planner therefore
/// cannot express a fixed-capacity device without a medium: an empty cold
/// start is a removable drive only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScsiAttachment {
    /// SCSI target ID.
    pub target: u8,
    /// Logical unit number.
    pub lun: u8,
    /// Device to install.
    pub device: ScsiDevice,
    medium: Option<ResourceId>,
}

impl ScsiAttachment {
    /// Plans a fixed-capacity device, which cannot start without its medium.
    pub(super) fn disk(target: u8, lun: u8, medium: ResourceId) -> Self {
        Self {
            target,
            lun,
            device: ScsiDevice::Disk,
            medium: Some(medium),
        }
    }

    /// Plans a removable device that may start with an empty slot.
    pub(super) fn cdrom(target: u8, lun: u8, medium: Option<ResourceId>) -> Self {
        Self {
            target,
            lun,
            device: ScsiDevice::Cdrom,
            medium,
        }
    }

    /// Returns the logical resource role of the cold-start medium.
    ///
    /// This is `None` only for a removable device that starts empty; a
    /// fixed-capacity device always requires a prepared medium.
    #[must_use]
    pub fn medium(&self) -> Option<&ResourceId> {
        self.medium.as_ref()
    }
}

/// A supported SCSI device selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScsiDevice {
    /// Writable SCSI disk.
    Disk,
    /// Read-only SCSI CD-ROM.
    Cdrom,
}

/// One configurable peripheral port on an Indigo IP12.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Ip12Port {
    /// SCC0 channel A keyboard port.
    Keyboard,
    /// SCC0 channel B mouse port.
    Mouse,
    /// SCC1 channel A external serial port.
    SerialA,
    /// SCC1 channel B external serial port.
    SerialB,
}

impl Ip12Port {
    /// Returns the stable runtime interface identity associated with this port.
    #[must_use]
    pub fn endpoint_key(self) -> EndpointKey {
        EndpointKey::new(match self {
            Self::Keyboard => "keyboard.0",
            Self::Mouse => "pointer.0",
            Self::SerialA => "serial.external.a",
            Self::SerialB => "serial.external.b",
        })
    }
}

/// A peripheral supported by one configurable IP12 port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ip12Peripheral {
    /// SGI keyboard connected to SCC0 channel A.
    SgiKeyboard,
    /// SGI mouse connected to SCC0 channel B.
    SgiMouse,
    /// Frontend-provided VT100 terminal connected to an external serial port.
    Vt100Terminal,
}

/// One validated IP12 port-to-peripheral attachment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ip12PortAttachment {
    port: Ip12Port,
    peripheral: Ip12Peripheral,
}

impl Ip12PortAttachment {
    pub(super) fn new(port: Ip12Port, peripheral: Ip12Peripheral) -> Self {
        assert!(
            matches!(
                (port, peripheral),
                (Ip12Port::Keyboard, Ip12Peripheral::SgiKeyboard)
                    | (Ip12Port::Mouse, Ip12Peripheral::SgiMouse)
                    | (
                        Ip12Port::SerialA | Ip12Port::SerialB,
                        Ip12Peripheral::Vt100Terminal
                    )
            ),
            "an IP12 build plan cannot contain an incompatible port peripheral"
        );
        Self { port, peripheral }
    }

    /// Returns the physical port receiving the peripheral.
    #[must_use]
    pub const fn port(&self) -> Ip12Port {
        self.port
    }

    /// Returns the peripheral attached to the port.
    #[must_use]
    pub const fn peripheral(&self) -> Ip12Peripheral {
        self.peripheral
    }
}

/// Semantic errors that prevent an IP12 draft from compiling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip12CompileError {
    pub(super) diagnostics: Vec<Diagnostic>,
}

impl Ip12CompileError {
    /// Returns the same semantic diagnostics emitted by resolution.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for Ip12CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "IP12 configuration has {} semantic error(s)",
            self.diagnostics.len()
        )
    }
}

impl Error for Ip12CompileError {}
