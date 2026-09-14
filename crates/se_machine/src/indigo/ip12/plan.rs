//! Validated, strongly typed construction requirements for an Indigo IP12.

use std::error::Error;
use std::fmt;

use se_config::diagnostic::Diagnostic;
use se_device::gio::GioSlot;
use se_float::backend::Backend;

use super::Ip12MemoryConfiguration;
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
    resources: ResourceRequirements,
}

impl Ip12BuildPlan {
    pub(super) fn new(
        floating_point_backend: Backend,
        memory: Ip12MemoryConfiguration,
        firmware: ResourceId,
        gio: Vec<GioAttachment>,
        scsi: Vec<ScsiAttachment>,
        resources: ResourceRequirements,
    ) -> Self {
        Self {
            floating_point_backend,
            memory,
            firmware,
            gio,
            scsi,
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

/// One independently addressed SCSI device and its medium role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScsiAttachment {
    /// SCSI target ID.
    pub target: u8,
    /// Logical unit number.
    pub lun: u8,
    /// Device to install.
    pub device: ScsiDevice,
    /// Logical resource role for this device's medium.
    pub medium: ResourceId,
}

/// A supported SCSI device selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScsiDevice {
    /// Writable SCSI disk.
    Disk,
    /// Read-only SCSI CD-ROM.
    Cdrom,
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
