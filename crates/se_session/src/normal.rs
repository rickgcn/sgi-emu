//! Normal session composition from a validated draft and host resources.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use se_config::diagnostic::{Diagnostic, DiagnosticSeverity};
use se_config::draft::MachineDraft;
use se_core::storage::StorageAccess;
use se_machine::indigo::ip12::builder::{self, Ip12AssemblyError};
use se_machine::indigo::ip12::definition::Ip12Definition;
use se_machine::indigo::ip12::plan::Ip12CompileError;
use se_machine::indigo::ip12::plan::{Ip12BuildPlan, PreparedIp12Build};
use se_machine::machine::Machine;
use se_machine::resource::{
    PrepareResourcesError, PreparedResource, ResourceKind, ResourceRequirement,
};
use se_network::config::NatConfig;

use crate::file_storage::HostFileStorage;
use crate::frontend::{FrontendPlan, SessionBuild};
use crate::persistence;

/// Failure to construct an ordinary machine session.
#[derive(Debug)]
pub enum NormalBuildError {
    /// The draft has semantic errors.
    Compile(Ip12CompileError),
    /// A host resource could not be prepared.
    Prepare(NormalPreparationError),
    /// Validated resources could not be assembled.
    Assemble(Ip12AssemblyError),
    /// Retained machine state could not be loaded.
    Persistence(Box<dyn Error>),
}

impl fmt::Display for NormalBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compile(error) => formatter.write_str(
                &error
                    .diagnostics()
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Self::Prepare(error) => error.fmt(formatter),
            Self::Assemble(error) => error.fmt(formatter),
            Self::Persistence(error) => error.fmt(formatter),
        }
    }
}

impl Error for NormalBuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Compile(error) => Some(error),
            Self::Prepare(error) => Some(error),
            Self::Assemble(error) => Some(error),
            Self::Persistence(error) => Some(error.as_ref()),
        }
    }
}

/// Compiles, prepares, assembles, and restores one Normal machine.
///
/// # Errors
///
/// Returns the failing semantic, resource, assembly, or persistence error.
pub fn build_configuration(
    draft: MachineDraft,
    network: NatConfig,
) -> Result<SessionBuild, NormalBuildError> {
    let plan = Ip12Definition
        .compile(&draft)
        .map_err(NormalBuildError::Compile)?;
    let frontend = FrontendPlan::from_ip12(&plan);
    let prepared = prepare_ip12(plan).map_err(NormalBuildError::Prepare)?;
    let mut machine =
        Machine::IndigoIp12(builder::build(prepared).map_err(NormalBuildError::Assemble)?);
    if let Some(restored) =
        persistence::load(&draft.model.0).map_err(NormalBuildError::Persistence)?
    {
        machine.restore_nonvolatile_state(restored.state, restored.offline_milliseconds);
    }
    Ok(SessionBuild::new(
        se_runtime::runtime::RuntimeConfiguration::normal_with_network(machine, network),
        frontend,
    ))
}

/// A host file could not be read or opened with the requested access.
#[derive(Debug)]
pub enum HostResourceError {
    /// Reading a complete byte image failed.
    ReadBytes {
        /// The path supplied by the build plan.
        path: PathBuf,
        /// The underlying host I/O error.
        source: io::Error,
    },
    /// Opening a fixed-capacity storage medium failed.
    OpenStorage {
        /// The path supplied by the build plan.
        path: PathBuf,
        /// The requested host file access.
        access: StorageAccess,
        /// The underlying host I/O error.
        source: io::Error,
    },
    /// Hashing a fixed-capacity storage medium failed.
    HashStorage {
        /// The path supplied by the build plan.
        path: PathBuf,
        /// The underlying host I/O error.
        source: io::Error,
    },
}

impl fmt::Display for HostResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadBytes { path, source } => {
                write!(
                    formatter,
                    "failed to read bytes from '{}': {source}",
                    path.display()
                )
            }
            Self::OpenStorage {
                path,
                access,
                source,
            } => write!(
                formatter,
                "failed to open {access} storage '{}': {source}",
                path.display()
            ),
            Self::HashStorage { path, source } => {
                write!(
                    formatter,
                    "failed to hash storage '{}': {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for HostResourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadBytes { source, .. }
            | Self::OpenStorage { source, .. }
            | Self::HashStorage { source, .. } => Some(source),
        }
    }
}

/// A provider or capability mismatch while preparing a validated IP12 plan.
pub type NormalPreparationError = PrepareResourcesError<HostResourceError>;

/// Prepares host-file capabilities and binds them to their exact IP12 plan.
///
/// This function neither assembles a machine nor validates file contents.
///
/// # Errors
///
/// Returns [`NormalPreparationError`] with the failing resource role and host
/// I/O error when any required file cannot be prepared.
pub fn prepare_ip12(plan: Ip12BuildPlan) -> Result<PreparedIp12Build, NormalPreparationError> {
    plan.prepare_with(|_, requirement| prepare_requirement(requirement))
}

/// Checks every host resource required by a semantically valid draft.
///
/// The same host capability operations used by authoritative preparation are
/// opened and immediately dropped. This function does not assemble a machine,
/// load persistence, or mutate runtime state.
#[must_use]
pub fn preflight_configuration(draft: &MachineDraft) -> Vec<Diagnostic> {
    let Ok(plan) = Ip12Definition.compile(draft) else {
        return Vec::new();
    };
    plan.resources()
        .iter()
        .filter_map(|(_, requirement)| {
            prepare_requirement(requirement)
                .err()
                .map(|error| host_resource_diagnostic(requirement, &error))
        })
        .collect()
}

fn prepare_requirement(
    requirement: &ResourceRequirement,
) -> Result<PreparedResource, HostResourceError> {
    let path = &requirement.path;
    match requirement.kind {
        ResourceKind::Bytes => fs::read(path)
            .map(PreparedResource::Bytes)
            .map_err(|source| HostResourceError::ReadBytes {
                path: path.clone(),
                source,
            }),
        ResourceKind::Storage { access } => {
            let storage = match access {
                StorageAccess::ReadOnly => HostFileStorage::open_read_only(path),
                StorageAccess::ReadWrite => HostFileStorage::open_read_write(path),
            }
            .map_err(|source| HostResourceError::OpenStorage {
                path: path.clone(),
                access,
                source,
            })?;
            Ok(PreparedResource::Storage {
                access,
                medium: Box::new(storage),
            })
        }
    }
}

fn host_resource_diagnostic(
    requirement: &ResourceRequirement,
    error: &HostResourceError,
) -> Diagnostic {
    let source = match error {
        HostResourceError::ReadBytes { source, .. }
        | HostResourceError::OpenStorage { source, .. }
        | HostResourceError::HashStorage { source, .. } => source,
    };
    let (code, message) = match source.kind() {
        io::ErrorKind::NotFound => (
            "host.resource.not-found",
            String::from("File does not exist."),
        ),
        io::ErrorKind::PermissionDenied => (
            "host.resource.permission-denied",
            match error {
                HostResourceError::OpenStorage {
                    access: StorageAccess::ReadWrite,
                    ..
                } => String::from("File cannot be opened for read-write access."),
                HostResourceError::ReadBytes { .. }
                | HostResourceError::OpenStorage {
                    access: StorageAccess::ReadOnly,
                    ..
                }
                | HostResourceError::HashStorage { .. } => String::from("File cannot be read."),
            },
        ),
        _ => (
            "host.resource.unavailable",
            match error {
                HostResourceError::ReadBytes { .. } | HostResourceError::HashStorage { .. } => {
                    format!("Could not read this file: {source}.")
                }
                HostResourceError::OpenStorage {
                    access: StorageAccess::ReadOnly,
                    ..
                } => format!("Could not open this file for read access: {source}."),
                HostResourceError::OpenStorage {
                    access: StorageAccess::ReadWrite,
                    ..
                } => format!("Could not open this file for read-write access: {source}."),
            },
        ),
    };
    Diagnostic {
        severity: DiagnosticSeverity::Error,
        code: code.into(),
        target: requirement.origin.clone(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use se_config::definition::MachineDefinition;
    use se_config::diagnostic::DiagnosticTarget;
    use se_config::draft::{Edit, MachineDraft};
    use se_config::id::{DeviceKindId, NodeId, PropertyId};
    use se_config::value::PropertyValue;
    use se_core::storage::StorageAccess;
    use se_machine::indigo::ip12::Ip12Error;
    use se_machine::indigo::ip12::builder::{Ip12AssemblyError, build};
    use se_machine::indigo::ip12::definition::Ip12Definition;
    use se_machine::indigo::ip12::plan::ScsiDevice;
    use se_machine::resource::{
        PrepareResourcesError, PreparedResource, ResourceKind, ResourceRequirement,
    };
    use se_network::config::NatConfig;
    use se_runtime::runtime::Runtime;

    use super::{
        HostResourceError, build_configuration, host_resource_diagnostic, preflight_configuration,
        prepare_ip12, prepare_requirement,
    };

    const PROM_BYTES: usize = 0x40000;
    static NEXT_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct TemporaryFiles {
        directory: PathBuf,
    }

    impl TemporaryFiles {
        fn new(name: &str) -> Self {
            let id = NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "sgi-emu-session-{name}-{}-{id}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            Self { directory }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.directory.join(name)
        }

        fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
            let path = self.path(name);
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for TemporaryFiles {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }

    fn set_path(draft: &mut MachineDraft, property: impl Into<String>, path: &Path) {
        draft.apply(Edit::SetProperty {
            property: PropertyId(property.into()),
            value: PropertyValue::Text(path.to_string_lossy().into_owned()),
        });
    }

    fn draft_with_firmware(path: &Path) -> MachineDraft {
        let mut draft = Ip12Definition.default_draft();
        set_path(&mut draft, "firmware.0.image-path", path);
        draft
    }

    fn attach_device(draft: &mut MachineDraft, target: u8, lun: u8, device: ScsiDevice) {
        let kind = match device {
            ScsiDevice::Disk => "scsi.disk",
            ScsiDevice::Cdrom => "scsi.cdrom",
        };
        draft.apply(Edit::SetAttachment {
            slot: NodeId(format!("scsi.0.target.{target}.lun.{lun}")),
            device: Some(DeviceKindId(kind.into())),
        });
    }

    fn set_medium_path(draft: &mut MachineDraft, target: u8, lun: u8, value: &str) {
        draft.apply(Edit::SetProperty {
            property: PropertyId(format!("scsi.0.target.{target}.lun.{lun}.medium-path")),
            value: PropertyValue::Text(value.into()),
        });
    }

    fn attach_medium(
        draft: &mut MachineDraft,
        target: u8,
        lun: u8,
        device: ScsiDevice,
        path: &Path,
    ) {
        attach_device(draft, target, lun, device);
        set_path(
            draft,
            format!("scsi.0.target.{target}.lun.{lun}.medium-path"),
            path,
        );
    }

    #[test]
    fn bytes_requirement_reads_the_complete_host_file() {
        let files = TemporaryFiles::new("bytes");
        let path = files.write("image.bin", [0, 1, 2, 3, 255]);
        let requirement = ResourceRequirement {
            path,
            kind: ResourceKind::Bytes,
            origin: DiagnosticTarget::Global,
        };
        let resource = prepare_requirement(&requirement).unwrap();
        assert!(matches!(resource, PreparedResource::Bytes(bytes) if bytes == [0, 1, 2, 3, 255]));
    }

    #[test]
    fn missing_bytes_preserve_resource_path_and_io_error() {
        let files = TemporaryFiles::new("missing-bytes");
        let path = files.path("missing.prom");
        let plan = Ip12Definition.compile(&draft_with_firmware(&path)).unwrap();
        let error = match prepare_ip12(plan) {
            Ok(_) => panic!("a missing PROM must fail host preparation"),
            Err(error) => error,
        };
        assert_eq!(error.resource_id().as_str(), "firmware.0.image");
        assert!(error.to_string().contains("firmware.0.image"));
        assert!(matches!(
            &error,
            PrepareResourcesError::Provider {
                source: HostResourceError::ReadBytes { path: failed_path, source },
                ..
            } if failed_path == &path && source.kind() == std::io::ErrorKind::NotFound
        ));
        assert!(error.source().unwrap().source().is_some());
    }

    #[test]
    fn missing_storage_reports_requested_access_and_path() {
        let files = TemporaryFiles::new("missing-storage");
        let firmware = files.write("prom.bin", vec![0; PROM_BYTES]);
        for (device, access, access_label) in [
            (ScsiDevice::Disk, StorageAccess::ReadWrite, "read-write"),
            (ScsiDevice::Cdrom, StorageAccess::ReadOnly, "read-only"),
        ] {
            let path = files.path(match device {
                ScsiDevice::Disk => "missing-disk.img",
                ScsiDevice::Cdrom => "missing-cd.iso",
            });
            let mut draft = draft_with_firmware(&firmware);
            attach_medium(&mut draft, 3, 5, device, &path);
            let plan = Ip12Definition.compile(&draft).unwrap();
            let error = match prepare_ip12(plan) {
                Ok(_) => panic!("missing storage must fail host preparation"),
                Err(error) => error,
            };
            assert_eq!(error.resource_id().as_str(), "scsi.0.target.3.lun.5.medium");
            assert!(error.to_string().contains(access_label));
            assert!(matches!(
                &error,
                PrepareResourcesError::Provider {
                    source: HostResourceError::OpenStorage {
                        path: failed_path,
                        access: failed_access,
                        source,
                    },
                    ..
                } if failed_path == &path
                    && *failed_access == access
                    && source.kind() == std::io::ErrorKind::NotFound
            ));
            assert!(error.source().unwrap().source().is_some());
        }
    }

    #[test]
    fn preflight_accepts_valid_resources_without_mutating_or_retaining_storage() {
        let files = TemporaryFiles::new("preflight-valid");
        let firmware = files.write("prom.bin", [1, 2, 3, 4]);
        let disk = files.write("disk.img", [5, 6, 7, 8]);
        let moved_disk = files.path("moved.img");
        let mut draft = draft_with_firmware(&firmware);
        attach_medium(&mut draft, 2, 0, ScsiDevice::Disk, &disk);

        assert!(preflight_configuration(&draft).is_empty());
        assert_eq!(fs::read(&disk).unwrap(), [5, 6, 7, 8]);
        fs::rename(&disk, &moved_disk).expect("preflight must release the storage handle");
        assert_eq!(fs::read(moved_disk).unwrap(), [5, 6, 7, 8]);
    }

    #[test]
    fn preflight_targets_missing_firmware_and_scsi_media() {
        let files = TemporaryFiles::new("preflight-targets");
        let missing_firmware = files.path("missing.prom");
        let missing_disk = files.path("missing.img");
        let missing_cdrom = files.path("missing.iso");
        let mut draft = draft_with_firmware(&missing_firmware);
        attach_medium(&mut draft, 2, 0, ScsiDevice::Disk, &missing_disk);
        attach_medium(&mut draft, 3, 4, ScsiDevice::Cdrom, &missing_cdrom);

        let diagnostics = preflight_configuration(&draft);
        assert_eq!(diagnostics.len(), 3);
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.code == "host.resource.not-found"
                && diagnostic.message == "File does not exist."
        }));
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.target.clone())
                .collect::<Vec<_>>(),
            [
                DiagnosticTarget::Property(PropertyId(String::from("firmware.0.image-path"))),
                DiagnosticTarget::Property(PropertyId(String::from(
                    "scsi.0.target.2.lun.0.medium-path"
                ))),
                DiagnosticTarget::Property(PropertyId(String::from(
                    "scsi.0.target.3.lun.4.medium-path"
                ))),
            ]
        );
    }

    #[test]
    fn preflight_skips_host_access_for_semantically_invalid_drafts() {
        let draft = Ip12Definition.default_draft();
        assert!(preflight_configuration(&draft).is_empty());
    }

    #[test]
    fn preflight_accepts_an_empty_cdrom_drive_without_opening_a_medium() {
        let files = TemporaryFiles::new("preflight-empty-cdrom");
        let firmware = files.write("prom.bin", vec![0; PROM_BYTES]);
        let mut draft = draft_with_firmware(&firmware);
        attach_device(&mut draft, 4, 0, ScsiDevice::Cdrom);
        set_medium_path(&mut draft, 4, 0, "");

        assert!(preflight_configuration(&draft).is_empty());

        let plan = Ip12Definition
            .compile(&draft)
            .expect("an empty CD-ROM drive is a complete topology");
        assert_eq!(plan.scsi().len(), 1);
        assert_eq!(plan.scsi()[0].medium(), None);
        assert_eq!(
            plan.resources()
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["firmware.0.image"],
            "an empty drive requires no host medium"
        );
    }

    #[test]
    fn an_empty_disk_path_stays_a_semantic_error() {
        let files = TemporaryFiles::new("empty-disk");
        let firmware = files.write("prom.bin", vec![0; PROM_BYTES]);
        let mut draft = draft_with_firmware(&firmware);
        attach_device(&mut draft, 2, 0, ScsiDevice::Disk);
        set_medium_path(&mut draft, 2, 0, "");

        assert!(
            Ip12Definition
                .resolve(&draft)
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "ip12.scsi.medium-required")
        );
        assert!(Ip12Definition.compile(&draft).is_err());
        assert!(
            preflight_configuration(&draft).is_empty(),
            "an invalid draft opens no host file"
        );
    }

    #[test]
    fn permission_diagnostics_follow_requested_capability() {
        let read_requirement = ResourceRequirement {
            path: PathBuf::from("firmware.bin"),
            kind: ResourceKind::Bytes,
            origin: DiagnosticTarget::Global,
        };
        let read_error = HostResourceError::ReadBytes {
            path: read_requirement.path.clone(),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let read = host_resource_diagnostic(&read_requirement, &read_error);
        assert_eq!(read.code, "host.resource.permission-denied");
        assert_eq!(read.message, "File cannot be read.");

        let write_requirement = ResourceRequirement {
            path: PathBuf::from("disk.img"),
            kind: ResourceKind::Storage {
                access: StorageAccess::ReadWrite,
            },
            origin: DiagnosticTarget::Global,
        };
        let write_error = HostResourceError::OpenStorage {
            path: write_requirement.path.clone(),
            access: StorageAccess::ReadWrite,
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let write = host_resource_diagnostic(&write_requirement, &write_error);
        assert_eq!(write.code, "host.resource.permission-denied");
        assert_eq!(
            write.message,
            "File cannot be opened for read-write access."
        );
    }

    #[test]
    fn invalid_prom_content_fails_only_during_assembly() {
        let files = TemporaryFiles::new("invalid-prom");
        let firmware = files.write("prom.bin", [0]);
        let plan = Ip12Definition
            .compile(&draft_with_firmware(&firmware))
            .unwrap();
        let prepared = prepare_ip12(plan).unwrap();
        assert!(matches!(
            build(prepared),
            Err(Ip12AssemblyError::BoardConstruction(
                Ip12Error::InvalidPromSize {
                    expected: PROM_BYTES,
                    actual: 1,
                }
            ))
        ));
    }

    #[test]
    fn invalid_disk_capacity_fails_only_during_assembly() {
        let files = TemporaryFiles::new("invalid-disk");
        let firmware = files.write("prom.bin", vec![0; PROM_BYTES]);
        let disk = files.write("disk.img", vec![0; 123]);
        let mut draft = draft_with_firmware(&firmware);
        attach_medium(&mut draft, 2, 3, ScsiDevice::Disk, &disk);
        let plan = Ip12Definition.compile(&draft).unwrap();
        let prepared = prepare_ip12(plan).unwrap();
        assert!(matches!(
            build(prepared),
            Err(Ip12AssemblyError::InvalidScsiMedium {
                target: 2,
                lun: 3,
                device: ScsiDevice::Disk,
                ..
            })
        ));
    }

    #[test]
    fn host_files_construct_dynamic_ip12_topology() {
        let files = TemporaryFiles::new("dynamic-topology");
        let firmware = files.write("prom.bin", vec![0; PROM_BYTES]);
        let disk_a = files.write("disk-a.img", vec![0; 512]);
        let disk_b = files.write("disk-b.img", vec![0; 1024]);
        let cdrom = files.write("cd.iso", vec![0; 2048]);
        let mut draft = draft_with_firmware(&firmware);
        attach_medium(&mut draft, 2, 3, ScsiDevice::Disk, &disk_a);
        attach_medium(&mut draft, 5, 1, ScsiDevice::Disk, &disk_b);
        attach_medium(&mut draft, 6, 4, ScsiDevice::Cdrom, &cdrom);

        let plan = Ip12Definition.compile(&draft).unwrap();
        assert_eq!(
            plan.scsi()
                .iter()
                .map(|attachment| (attachment.target, attachment.lun, attachment.device))
                .collect::<Vec<_>>(),
            [
                (2, 3, ScsiDevice::Disk),
                (5, 1, ScsiDevice::Disk),
                (6, 4, ScsiDevice::Cdrom),
            ]
        );
        let prepared = prepare_ip12(plan).unwrap();
        let machine = build(prepared).unwrap();
        assert!(
            machine
                .endpoint_catalog()
                .endpoints()
                .iter()
                .any(|endpoint| { endpoint.kind() == se_machine::endpoint::EndpointKind::Video })
        );
    }

    #[test]
    fn normal_session_build_keeps_runtime_and_frontend_from_one_plan() {
        let files = TemporaryFiles::new("normal-frontend");
        let firmware = files.write("prom.bin", vec![0; PROM_BYTES]);
        let mut draft = draft_with_firmware(&firmware);
        draft.apply(Edit::SetAttachment {
            slot: NodeId(String::from("serial.1.channel.b.port")),
            device: None,
        });

        let (configuration, frontend) = build_configuration(draft, NatConfig::default())
            .unwrap()
            .into_parts();
        assert_eq!(
            frontend
                .serial_console_endpoints()
                .iter()
                .map(|endpoint| endpoint.as_str())
                .collect::<Vec<_>>(),
            ["serial.external.a"]
        );

        let runtime = Runtime::new_unconfigured().unwrap();
        runtime.configure_with(configuration).unwrap();
        assert_eq!(
            runtime
                .endpoint_catalog()
                .unwrap()
                .endpoints()
                .iter()
                .filter(|endpoint| {
                    endpoint.kind() == se_machine::endpoint::EndpointKind::Serial
                })
                .count(),
            2
        );
        runtime.shutdown().unwrap();
    }
}
